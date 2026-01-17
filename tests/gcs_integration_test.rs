use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn sy_bin() -> String {
    env!("CARGO_BIN_EXE_sy").to_string()
}

fn setup_test_dir(name: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    // Create some structure
    fs::write(dir.path().join(format!("{}_file1.txt", name)), "content1").unwrap();
    fs::create_dir_all(dir.path().join("subdir")).unwrap();
    fs::write(
        dir.path().join(format!("subdir/{}_file2.txt", name)),
        "content2",
    )
    .unwrap();
    dir
}

// Helper to get GCS configuration from environment
struct GcsConfig {
    bucket: String,
    project_id: Option<String>,
    service_account_path: Option<String>,
    prefix: String,
}

impl GcsConfig {
    fn from_env() -> Option<Self> {
        let bucket = std::env::var("SY_TEST_GCS_BUCKET")
            .or_else(|_| std::env::var("GCP_BUCKET_NAME"))
            .ok()?;
        let project_id = std::env::var("SY_TEST_GCS_PROJECT")
            .or_else(|_| std::env::var("GCP_PROJECT_ID"))
            .ok();
        let service_account_path = std::env::var("SY_TEST_GCS_SERVICE_ACCOUNT")
            .or_else(|_| std::env::var("GOOGLE_APPLICATION_CREDENTIALS"))
            .ok();
        // Use a random prefix to avoid collisions
        let prefix = format!(
            "sy-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs()
        );

        Some(Self {
            bucket,
            project_id,
            service_account_path,
            prefix,
        })
    }

    fn to_url(&self, path: &str) -> String {
        self.to_url_with_trailing_slash(path, false)
    }

    fn to_url_with_trailing_slash(&self, path: &str, trailing_slash: bool) -> String {
        let key = if path.is_empty() {
            self.prefix.clone()
        } else {
            format!("{}/{}", self.prefix, path)
        };

        let slash = if trailing_slash { "/" } else { "" };
        let mut url = format!("gs://{}/{}{}", self.bucket, key, slash);
        let mut params = Vec::new();

        if let Some(ref project) = self.project_id {
            params.push(format!("project={}", project));
        }
        if let Some(ref sa) = self.service_account_path {
            params.push(format!("service_account={}", sa));
        }

        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        url
    }
}

#[test]
fn test_gcs_upload_download() {
    let config = match GcsConfig::from_env() {
        Some(c) => c,
        None => {
            println!("Skipping GCS test: SY_TEST_GCS_BUCKET or GCP_BUCKET_NAME not set");
            return;
        }
    };

    let source = setup_test_dir("upload");
    let dest = TempDir::new().unwrap();

    println!("Testing GCS sync with prefix: {}", config.prefix);

    // 1. Sync Local -> GCS
    let gcs_url = config.to_url(""); // Root of prefix
    let output = Command::new(sy_bin())
        .args([
            &format!("{}/", source.path().display()),
            &gcs_url,
            "--use-cache=false", // Disable cache to force scan
        ])
        .output()
        .expect("Failed to run sy");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        panic!(
            "Local -> GCS sync failed\nStderr: {}\nStdout: {}",
            stderr, stdout
        );
    }

    // 2. Verify files were uploaded by listing GCS (optional - requires gcloud CLI)
    println!("Verifying files uploaded to GCS...");
    let mut gcloud_args = vec![
        "storage".to_string(),
        "ls".to_string(),
        "--recursive".to_string(),
        format!("gs://{}/{}/", config.bucket, config.prefix),
    ];
    if let Some(ref project) = config.project_id {
        gcloud_args.push("--project".to_string());
        gcloud_args.push(project.clone());
    }
    let output = std::process::Command::new("gcloud")
        .args(&gcloud_args)
        .output();

    if let Ok(output) = output {
        let listing = String::from_utf8_lossy(&output.stdout);
        println!("GCS listing:\n{}", listing);

        assert!(
            listing.contains("upload_file1.txt"),
            "Expected upload_file1.txt in GCS listing"
        );
        assert!(
            listing.contains("upload_file2.txt"),
            "Expected upload_file2.txt in GCS listing"
        );
    } else {
        println!("Note: gcloud CLI not available, skipping listing verification");
    }

    // 3. Sync GCS -> Local (download)
    println!("Testing GCS -> Local sync...");
    // Use trailing slash to copy contents (rsync semantics)
    let gcs_url_download = config.to_url_with_trailing_slash("", true);
    println!("Download URL: {}", gcs_url_download);
    let output = Command::new(sy_bin())
        .args([
            &gcs_url_download,
            dest.path().to_str().unwrap(),
            "--use-cache=false",
        ])
        .output()
        .expect("Failed to run sy");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("GCS -> Local stdout:\n{}", stdout);
    if !stderr.is_empty() {
        println!("GCS -> Local stderr:\n{}", stderr);
    }

    if !output.status.success() {
        panic!(
            "GCS -> Local sync failed\nStderr: {}\nStdout: {}",
            stderr, stdout
        );
    }

    // 4. Verify downloaded files
    let file1_path = dest.path().join("upload_file1.txt");
    let file2_path = dest.path().join("subdir/upload_file2.txt");

    assert!(
        file1_path.exists(),
        "Expected upload_file1.txt to exist at {:?}",
        file1_path
    );
    assert!(
        file2_path.exists(),
        "Expected subdir/upload_file2.txt to exist at {:?}",
        file2_path
    );

    // Verify content
    assert_eq!(
        fs::read_to_string(&file1_path).unwrap(),
        "content1",
        "File content mismatch for upload_file1.txt"
    );
    assert_eq!(
        fs::read_to_string(&file2_path).unwrap(),
        "content2",
        "File content mismatch for subdir/upload_file2.txt"
    );

    println!("GCS -> Local sync successful!");

    // 5. Cleanup (Delete from GCS)
    // We can use --delete with an empty source to delete everything in the prefix
    let empty_source = TempDir::new().unwrap();
    let output = Command::new(sy_bin())
        .args([
            &format!("{}/", empty_source.path().display()),
            &gcs_url,
            "--delete",
            "--use-cache=false",
            "--force-delete", // Force delete since we're deleting everything
        ])
        .output()
        .expect("Failed to run sy cleanup");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("Warning: Failed to cleanup GCS: {}", stderr);
    }
}

#[test]
fn test_gcs_multipart_upload() {
    let config = match GcsConfig::from_env() {
        Some(c) => c,
        None => {
            println!("Skipping GCS test: SY_TEST_GCS_BUCKET or GCP_BUCKET_NAME not set");
            return;
        }
    };

    let source = TempDir::new().unwrap();
    let large_file_name = "large_file.bin";
    let large_file_path = source.path().join(large_file_name);

    // Create a 6MB file (threshold is 5MB)
    let size = 6 * 1024 * 1024;
    let data = vec![b'A'; size];
    fs::write(&large_file_path, &data).unwrap();

    println!(
        "Testing GCS multipart upload with prefix: {}",
        config.prefix
    );

    // 1. Sync Local -> GCS
    let gcs_url = config.to_url("");
    let output = Command::new(sy_bin())
        .args([
            &format!("{}/", source.path().display()),
            &gcs_url,
            "--use-cache=false",
        ])
        .output()
        .expect("Failed to run sy");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!("Local -> GCS sync failed: {}", stderr);
    }

    // 2. Verify the large file was uploaded by listing GCS (optional - requires gcloud CLI)
    println!("Verifying large file uploaded to GCS...");
    let mut gcloud_args = vec![
        "storage".to_string(),
        "ls".to_string(),
        "-l".to_string(),
        format!("gs://{}/{}/", config.bucket, config.prefix),
    ];
    if let Some(ref project) = config.project_id {
        gcloud_args.push("--project".to_string());
        gcloud_args.push(project.clone());
    }
    let output = std::process::Command::new("gcloud")
        .args(&gcloud_args)
        .output();

    if let Ok(output) = output {
        let listing = String::from_utf8_lossy(&output.stdout);
        println!("GCS listing:\n{}", listing);

        // Check that the large file exists in the listing
        assert!(
            listing.contains(large_file_name),
            "Expected {} in GCS listing",
            large_file_name
        );

        // Verify file size is correct (6MB = 6291456 bytes)
        assert!(
            listing.contains("6291456") || listing.contains("6.0 MiB"),
            "Expected 6MB file size in GCS listing"
        );
    } else {
        println!("Note: gcloud CLI not available, skipping listing verification");
    }

    // 3. Cleanup
    let empty_source = TempDir::new().unwrap();
    let output = Command::new(sy_bin())
        .args([
            &format!("{}/", empty_source.path().display()),
            &gcs_url,
            "--delete",
            "--use-cache=false",
            "--force-delete",
        ])
        .output()
        .expect("Failed to run sy cleanup");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("Warning: Failed to cleanup GCS: {}", stderr);
    }
}

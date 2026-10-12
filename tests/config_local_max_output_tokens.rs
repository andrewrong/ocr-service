use std::process::Command;

/// Starts the binary, which reads the configuration first, with the setting in its environment.
fn start_with(value: &str) -> (bool, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_ocr-service"))
        .args(["serve", "--port", "0"])
        .env("OCR_LOCAL_MAX_OUTPUT_TOKENS", value)
        .output()
        .expect("binary should start");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn zero_is_rejected_when_the_configuration_is_read() {
    let (success, stderr) = start_with("0");
    assert!(!success);
    assert!(stderr.contains("OCR_LOCAL_MAX_OUTPUT_TOKENS"), "{stderr}");
}

#[test]
fn a_non_number_is_rejected_when_the_configuration_is_read() {
    let (success, stderr) = start_with("lots");
    assert!(!success);
    assert!(stderr.contains("OCR_LOCAL_MAX_OUTPUT_TOKENS"), "{stderr}");
}

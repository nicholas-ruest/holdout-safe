use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn fixture() -> &'static str {
    concat!(
        "{\"patient\":\"a\",\"label\":\"case\",\"visit\":1}\n",
        "{\"patient\":\"a\",\"label\":\"case\",\"visit\":2}\n",
        "{\"patient\":\"b\",\"label\":\"control\",\"visit\":1}\n",
        "{\"patient\":\"c\",\"label\":\"case\",\"visit\":1}\n",
        "{\"patient\":\"d\",\"label\":\"control\",\"visit\":1}\n",
    )
}

#[test]
fn split_then_audit_round_trip() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("records.jsonl");
    let output = temp.path().join("folds");
    fs::write(&input, fixture()).unwrap();

    let split = Command::new(env!("CARGO_BIN_EXE_holdout-safe"))
        .args([
            "split",
            "--input",
            input.to_str().unwrap(),
            "--group-field",
            "patient",
            "--label-field",
            "label",
            "--folds",
            "2",
            "--seed",
            "integration",
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        split.status.success(),
        "{}",
        String::from_utf8_lossy(&split.stderr)
    );
    assert!(output.join("fold-000.jsonl").is_file());
    assert!(output.join("fold-001.jsonl").is_file());

    let audit = Command::new(env!("CARGO_BIN_EXE_holdout-safe"))
        .args([
            "audit",
            "--input",
            input.to_str().unwrap(),
            "--manifest",
            output.join("manifest.json").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(audit.status.success());
    assert!(String::from_utf8_lossy(&audit.stdout).starts_with("VALID:"));
}

#[test]
fn audit_uses_exit_two_for_a_mismatch() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("records.jsonl");
    let manifest = temp.path().join("manifest.json");
    fs::write(&input, fixture()).unwrap();

    let plan = Command::new(env!("CARGO_BIN_EXE_holdout-safe"))
        .args([
            "plan",
            "--input",
            input.to_str().unwrap(),
            "--group-field",
            "patient",
            "--folds",
            "2",
        ])
        .output()
        .unwrap();
    assert!(plan.status.success());
    fs::write(&manifest, &plan.stdout).unwrap();
    fs::write(
        &input,
        format!("{}{{\"patient\":\"new\",\"label\":\"case\"}}\n", fixture()),
    )
    .unwrap();

    let audit = Command::new(env!("CARGO_BIN_EXE_holdout-safe"))
        .args([
            "audit",
            "--input",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(audit.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&audit.stdout).contains("digest"));
}

#[test]
fn split_refuses_a_nonempty_output_directory() {
    let temp = tempdir().unwrap();
    let input = temp.path().join("records.jsonl");
    let output = temp.path().join("folds");
    fs::write(&input, fixture()).unwrap();
    fs::create_dir(&output).unwrap();
    fs::write(output.join("keep.txt"), "do not overwrite").unwrap();

    let result = Command::new(env!("CARGO_BIN_EXE_holdout-safe"))
        .args([
            "split",
            "--input",
            input.to_str().unwrap(),
            "--group-field",
            "patient",
            "--out-dir",
            output.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    assert_eq!(
        fs::read_to_string(output.join("keep.txt")).unwrap(),
        "do not overwrite"
    );
}

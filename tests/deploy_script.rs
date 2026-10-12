//! Behaviour of `scripts/deploy.sh`, run against a fake `docker` and a fake `git` put first on
//! `PATH`. No test reaches a Docker daemon or a running service.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const OLD: &str = "fedcba9876543210fedcba9876543210fedcba98";

const FAKE_DOCKER: &str = r#"#!/usr/bin/env bash
echo "docker $*" >>"$FAKE_LOG"
case "$1" in
  image)
    name=${@: -1}
    [[ $name == ocr-service:local && -n ${FAKE_NO_LOCAL:-} ]] && exit 1
    for m in ${FAKE_MISSING:-}; do [[ $m == "$name" ]] && exit 1; done
    [[ $2 == inspect && $3 == --format ]] && echo "${FAKE_LABEL:-}"
    exit 0 ;;
  build) cat >/dev/null; exit "${FAKE_BUILD_FAIL:-0}" ;;
  tag) exit 0 ;;
  compose)
    echo "compose in $PWD: $*" >>"$FAKE_LOG"
    [[ $2 == up ]] && exit "${FAKE_UP_FAIL:-0}"
    [[ $2 == ps ]] && echo container1
    exit 0 ;;
  inspect) echo "${FAKE_HEALTH:-healthy}"; exit 0 ;;
esac
exit 0
"#;

const FAKE_GIT: &str = r#"#!/usr/bin/env bash
case "$3" in
  fetch) exit "${FAKE_FETCH_FAIL:-0}" ;;
  rev-parse) [[ ${@: -1} == nonsense* ]] && exit 1; echo "$FAKE_COMMIT"; exit 0 ;;
  archive) echo tar; exit 0 ;;
esac
exit 1
"#;

struct Sandbox {
    dir: PathBuf,
    env: Vec<(String, String)>,
}

impl Sandbox {
    fn new() -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "deploy-script-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("bin")).unwrap();
        fs::create_dir_all(dir.join("service")).unwrap();
        for (name, body) in [("docker", FAKE_DOCKER), ("git", FAKE_GIT)] {
            let p = dir.join("bin").join(name);
            fs::write(&p, body).unwrap();
            Command::new("chmod").arg("+x").arg(&p).status().unwrap();
        }
        let env = vec![
            (
                "FAKE_LOG".into(),
                dir.join("calls.log").display().to_string(),
            ),
            ("FAKE_COMMIT".into(), COMMIT.into()),
            ("FAKE_LABEL".into(), OLD.into()),
        ];
        Sandbox { dir, env }
    }

    fn set(mut self, k: &str, v: &str) -> Self {
        self.env.push((k.into(), v.into()));
        self
    }

    fn record(&self) -> PathBuf {
        self.dir.join("record")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_with(args, true, true)
    }

    fn run_with(&self, args: &[&str], service: bool, record: bool) -> Output {
        let path = format!(
            "{}:{}",
            self.dir.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let mut cmd = Command::new("bash");
        cmd.arg(concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/deploy.sh"))
            .args(args)
            .env("PATH", path)
            .env("OCR_DEPLOY_HEALTH_TIMEOUT", "2");
        if service {
            cmd.env("OCR_DEPLOY_SERVICE_DIR", self.dir.join("service"));
        }
        if record {
            cmd.env("OCR_DEPLOY_RECORD", self.record());
        }
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.dir.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    fn lines(&self) -> Vec<String> {
        fs::read_to_string(self.record())
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn position(calls: &[String], needle: &str) -> usize {
    calls
        .iter()
        .position(|c| c.contains(needle))
        .unwrap_or_else(|| panic!("no call containing {needle:?} in {calls:#?}"))
}

fn retags_of_local(calls: &[String]) -> usize {
    calls
        .iter()
        .filter(|c| c.starts_with("docker tag ") && c.ends_with(" ocr-service:local"))
        .count()
}

fn is_record_line(line: &str, action: &str, result: &str) -> bool {
    let f: Vec<&str> = line.split(' ').collect();
    let hex = |s: &str| {
        (7..=40).contains(&s.len()) && s.bytes().all(|b| b"0123456789abcdef".contains(&b))
    };
    f.len() == 5
        && f[0].len() == 25
        && f[0].as_bytes()[19] != b'.'
        && (f[0].as_bytes()[19] == b'+' || f[0].as_bytes()[19] == b'-')
        && f[0].as_bytes()[22] == b':'
        && f[1] == action
        && hex(f[2])
        && hex(f[3])
        && f[4] == result
}

#[test]
fn a_good_deploy_tags_builds_retags_and_starts_in_order() {
    let sb = Sandbox::new();
    let out = sb.run(&[]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let calls = sb.calls();
    let old_tag = position(
        &calls,
        &format!("docker tag ocr-service:local ocr-service:{OLD}"),
    );
    let build = position(&calls, "docker build");
    assert!(calls[build].contains(&format!(
        "--label org.opencontainers.image.revision={COMMIT}"
    )));
    assert!(calls[build].contains(&format!("-t ocr-service:{COMMIT}")));
    let retag = position(
        &calls,
        &format!("docker tag ocr-service:{COMMIT} ocr-service:local"),
    );
    let up = position(&calls, "compose in ");
    assert!(calls[up].contains(&sb.dir.join("service").display().to_string()));
    assert!(calls[up].ends_with("compose up -d --no-build"));
    assert!(old_tag < build && build < retag && retag < up);
    let lines = sb.lines();
    assert_eq!(lines.len(), 1);
    assert!(is_record_line(&lines[0], "deploy", "ok"), "{lines:?}");
    assert!(lines[0].contains(&format!(" {OLD} {COMMIT} ok")));
}

#[test]
fn a_first_deploy_records_the_unknown_commit_as_from() {
    let sb = Sandbox::new().set("FAKE_LABEL", "");
    let out = sb.run(&[]);
    assert_eq!(out.status.code(), Some(0));
    let lines = sb.lines();
    assert_eq!(lines.len(), 1);
    assert!(
        lines[0].contains(&format!(" 0000000 {COMMIT} ok")),
        "{lines:?}"
    );
}

#[test]
fn a_failed_build_is_recorded_and_exits_1() {
    let sb = Sandbox::new().set("FAKE_BUILD_FAIL", "1");
    assert_eq!(sb.run(&[]).status.code(), Some(1));
    let lines = sb.lines();
    assert_eq!(lines.len(), 1);
    assert!(
        is_record_line(&lines[0], "deploy", "build-failed"),
        "{lines:?}"
    );
    assert_eq!(retags_of_local(&sb.calls()), 0);
}

#[test]
fn a_failed_compose_up_is_recorded_and_exits_1() {
    let sb = Sandbox::new().set("FAKE_UP_FAIL", "1");
    assert_eq!(sb.run(&[]).status.code(), Some(1));
    let lines = sb.lines();
    assert_eq!(lines.len(), 1);
    assert!(
        is_record_line(&lines[0], "deploy", "up-failed"),
        "{lines:?}"
    );
}

#[test]
fn a_container_that_never_gets_healthy_is_unhealthy_and_not_rolled_back() {
    let sb = Sandbox::new().set("FAKE_HEALTH", "starting");
    assert_eq!(sb.run(&[]).status.code(), Some(1));
    let lines = sb.lines();
    assert_eq!(lines.len(), 1);
    assert!(
        is_record_line(&lines[0], "deploy", "unhealthy"),
        "{lines:?}"
    );
    let calls = sb.calls();
    let up = position(&calls, "compose in ");
    assert!(calls[up..].iter().all(|c| !c.starts_with("docker tag ")));
}

#[test]
fn a_failed_fetch_and_an_unknown_commit_are_recorded() {
    let sb = Sandbox::new().set("FAKE_FETCH_FAIL", "1");
    assert_eq!(sb.run(&[]).status.code(), Some(1));
    assert!(is_record_line(&sb.lines()[0], "deploy", "fetch-failed"));

    let sb = Sandbox::new();
    assert_eq!(sb.run(&["nonsense"]).status.code(), Some(1));
    let lines = sb.lines();
    assert!(
        is_record_line(&lines[0], "deploy", "unknown-commit"),
        "{lines:?}"
    );
    assert!(sb.calls().iter().all(|c| !c.starts_with("docker build")));
}

#[test]
fn a_rollback_points_local_at_the_from_of_the_last_ok_deploy() {
    let sb = Sandbox::new().set("FAKE_LABEL", COMMIT);
    fs::write(
        sb.record(),
        format!(
            "2026-10-12T18:30:00+08:00 deploy 0000000 {OLD} ok\n\
             2026-10-12T19:30:00+08:00 deploy {OLD} {COMMIT} ok\n\
             2026-10-12T20:30:00+08:00 deploy {COMMIT} {COMMIT} unhealthy\n"
        ),
    )
    .unwrap();
    let out = sb.run(&["--rollback"]);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let calls = sb.calls();
    assert!(calls.contains(&format!("docker tag ocr-service:{OLD} ocr-service:local")));
    assert!(calls.iter().all(|c| !c.starts_with("docker build")));
    position(&calls, "compose in ");
    let lines = sb.lines();
    assert_eq!(lines.len(), 4);
    assert!(is_record_line(&lines[3], "rollback", "ok"), "{lines:?}");
    assert!(lines[3].contains(&format!(" {COMMIT} {OLD} ok")));
}

#[test]
fn a_rollback_without_a_target_or_without_its_image_fails() {
    let sb = Sandbox::new();
    assert_eq!(sb.run(&["--rollback"]).status.code(), Some(1));
    assert!(is_record_line(
        &sb.lines()[0],
        "rollback",
        "no-rollback-target"
    ));
    assert_eq!(retags_of_local(&sb.calls()), 0);

    let sb = Sandbox::new().set("FAKE_MISSING", &format!("ocr-service:{OLD}"));
    fs::write(
        sb.record(),
        format!("2026-10-12T19:30:00+08:00 deploy {OLD} {COMMIT} ok\n"),
    )
    .unwrap();
    assert_eq!(sb.run(&["--rollback"]).status.code(), Some(1));
    let lines = sb.lines();
    assert_eq!(lines.len(), 2);
    assert!(
        is_record_line(&lines[1], "rollback", "image-missing"),
        "{lines:?}"
    );
    assert_eq!(retags_of_local(&sb.calls()), 0);
}

#[test]
fn a_missing_variable_or_bad_arguments_exit_2_and_write_no_record() {
    let sb = Sandbox::new();
    assert_eq!(sb.run_with(&[], true, false).status.code(), Some(2));
    assert_eq!(sb.run_with(&[], false, true).status.code(), Some(2));
    assert_eq!(sb.run(&["a", "b"]).status.code(), Some(2));
    assert_eq!(sb.run(&["--bogus"]).status.code(), Some(2));
    assert!(!Path::new(&sb.record()).exists());
}

#[test]
fn a_dot_env_file_is_never_printed() {
    let sb = Sandbox::new();
    fs::write(sb.dir.join("service/.env"), "SECRET=marker-4f9a2c\n").unwrap();
    let out = sb.run(&[]);
    assert_eq!(out.status.code(), Some(0));
    let printed = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!printed.contains("marker-4f9a2c"));
}

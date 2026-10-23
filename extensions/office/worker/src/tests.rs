use super::*;

// A separate process using the test executable keeps these supervisor tests
// portable and independent of an installed Office engine or shell.
#[test]
#[ignore = "subprocess fixture for the supervisor tests"]
fn engine_stub() {
    let work = PathBuf::from(std::env::var_os("RECAST_TEST_OFFICE_WORK").unwrap());
    let count = work.join("starts");
    let attempt = fs::read_to_string(&count)
        .unwrap_or_default()
        .parse::<usize>()
        .unwrap_or(0);
    fs::write(count, (attempt + 1).to_string()).unwrap();
    let plan = std::env::var("RECAST_TEST_OFFICE_PLAN").unwrap();
    let steps: Vec<_> = plan.split(',').collect();
    match steps[attempt.min(steps.len() - 1)] {
        "cancel" => {
            fs::write(work.join("cancel"), b"").unwrap();
            std::process::exit(81);
        }
        "hang" => thread::sleep(Duration::from_secs(30)),
        code => std::process::exit(code.parse().unwrap()),
    }
}

fn run_stub(plan: &str, timeout: Duration) -> (Result<ExitStatus>, tempfile::TempDir) {
    let directory = tempfile::tempdir().unwrap();
    let work = directory.path();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--ignored", "--exact", "tests::engine_stub", "--nocapture"])
        .env("RECAST_TEST_OFFICE_WORK", work)
        .env("RECAST_TEST_OFFICE_PLAN", plan)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let result = run_office(&mut command, work, &work.join("office.log"), timeout);
    (result, directory)
}

fn starts(work: &Path) -> usize {
    fs::read_to_string(work.join("starts"))
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn first_run_restart_preserves_workspace_and_records_both_exits() {
    let (result, directory) = run_stub("81,0", Duration::from_secs(10));
    assert!(result.unwrap().success());
    assert_eq!(starts(directory.path()), 2);
    let diagnostics: Value =
        serde_json::from_slice(&fs::read(directory.path().join("office-process.json")).unwrap())
            .unwrap();
    assert_eq!(diagnostics["attempts"][0]["exitCode"], 81);
    assert_eq!(diagnostics["attempts"][1]["exitCode"], 0);
}

#[test]
fn ordinary_failures_and_crashes_are_not_retried() {
    for code in [1, 77, 79] {
        let (result, directory) = run_stub(&format!("{code},0"), Duration::from_secs(10));
        assert_eq!(result.unwrap().code(), Some(code));
        assert_eq!(starts(directory.path()), 1);
    }
}

#[test]
fn repeated_restart_is_bounded() {
    let (result, directory) = run_stub("81", Duration::from_secs(10));
    assert_eq!(result.unwrap().code(), Some(81));
    assert_eq!(starts(directory.path()), 2);
}

#[test]
fn cancellation_prevents_restart() {
    let (result, directory) = run_stub("cancel,0", Duration::from_secs(10));
    assert_eq!(result.unwrap_err(), "Cancelled.");
    assert_eq!(starts(directory.path()), 1);
}

#[test]
fn restarted_engine_is_still_subject_to_the_deadline() {
    let (result, directory) = run_stub("81,hang", Duration::from_secs(2));
    assert_eq!(
        result.unwrap_err(),
        "This document took too long to render."
    );
    assert_eq!(starts(directory.path()), 2);
}

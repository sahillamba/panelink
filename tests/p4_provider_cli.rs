#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use panelink::control_protocol::TargetRef;
use panelink::provider_binding::{Binding, ProjectIdentity, SupportedVersion, VerificationSource};
use panelink::provider_binding_store::ProviderBindingStore;
use std::fs;
use std::os::unix::{ffi::OsStrExt, fs::DirBuilderExt};
use std::process::Command;

#[test]
fn historical_cli_requires_exact_target_project_and_available_store() {
    let root = std::env::temp_dir().join(format!("pl-p4-cli-{}", std::process::id()));
    fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let project = root.join("project");
    let foreign = root.join("foreign");
    let bindings = root.join("provider-bindings");
    let slot = bindings.join("1-2-3-4-5");
    for path in [&project, &foreign, &bindings, &slot] {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    let target = TargetRef {
        generation: 1,
        workspace: 2,
        pane: 3,
        session: 4,
        occupant_revision: 5,
    };
    let project_identity = ProjectIdentity::new(project.as_os_str().as_bytes()).unwrap();
    let binding = Binding::new(
        target,
        &project_identity,
        &SupportedVersion::new("2.0.32").unwrap(),
        6,
        7,
        "session_explicit",
        8,
        VerificationSource::TuiRouteSnapshot,
    )
    .unwrap();
    let mut store =
        ProviderBindingStore::open(&slot, target, project_identity.as_bytes(), "2.0.32").unwrap();
    store.save(&binding).unwrap();
    let call = |target: &str, project: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_panelink"))
            .args(["provider-binding", "show", "--directory"])
            .arg(&root)
            .args(["--target", target, "--project"])
            .arg(project)
            .output()
            .unwrap()
    };
    // Active storage ownership cannot be bypassed by a separate inspector.
    assert!(!call("1:2:3:4:5", &project).status.success());
    drop(store);
    let result = call("1:2:3:4:5", &project);
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("\"state\":\"last_confirmed_conversation\""));
    assert!(text.contains("\"provider_session_id\":\"session_explicit\""));
    assert!(text.contains("\"current_at_crash\":false"));
    assert!(text.contains("\"automatic_resume\":false"));
    for (target, project) in [
        ("1:2:3:4:6", project.as_path()),
        ("latest", project.as_path()),
        ("1:2:3:4:5", foreign.as_path()),
    ] {
        assert!(!call(target, project).status.success());
    }
    assert!(!bindings.join("1-2-3-4-6").exists());
    fs::remove_dir_all(&root).unwrap();
}

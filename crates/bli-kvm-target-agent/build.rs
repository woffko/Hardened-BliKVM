//! On Windows the agent's programs carry its icon (`assets/agent.ico`, made
//! by `tools/build_agent_icon.mjs`) and their version and commit in their
//! resources. The resource script is written here and compiled with LLVM's
//! resource compiler: the one `BLI_KVM_RC` names, or `llvm-rc` on the path.
//! Other targets build nothing here.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/agent.ico");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=BLI_KVM_SOURCE_COMMIT");
    println!("cargo:rerun-if-env-changed=BLI_KVM_RC");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(env::var("CARGO_MANIFEST_DIR").expect("the manifest's directory"));
    let out = PathBuf::from(env::var("OUT_DIR").expect("the build's directory"));
    let version = env::var("CARGO_PKG_VERSION").expect("the package's version");
    let commit = match env::var("BLI_KVM_SOURCE_COMMIT") {
        Ok(commit) if !commit.is_empty() => commit,
        _ => "unknown".into(),
    };
    let mut numbers: Vec<u16> = version
        .split(['.', '-'])
        .take(3)
        .map(|part| part.parse().unwrap_or(0))
        .collect();
    numbers.resize(4, 0);
    let numbers = numbers
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let quoted = |text: &str| text.replace('\\', "\\\\").replace('"', "\"\"");
    let icon = manifest.join("assets").join("agent.ico");
    let script = format!(
        r#"1 ICON "{icon}"
1 VERSIONINFO
FILEVERSION {numbers}
PRODUCTVERSION {numbers}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Hardened BliKVM"
      VALUE "FileDescription", "Hardened BliKVM agent"
      VALUE "FileVersion", "{version}"
      VALUE "ProductName", "Hardened BliKVM"
      VALUE "ProductVersion", "{version} ({commit})"
      VALUE "Comments", "Built from commit {commit}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = quoted(&icon.to_string_lossy()),
        version = quoted(&version),
        commit = quoted(&commit),
    );
    let rc = out.join("agent.rc");
    let res = out.join("agent.res");
    fs::write(&rc, script).expect("the resource script is written");
    let compilers: Vec<String> = match env::var("BLI_KVM_RC") {
        Ok(compiler) if !compiler.is_empty() => vec![compiler],
        _ => ["llvm-rc", "llvm-rc-19", "llvm-rc-18"]
            .iter()
            .map(|name| name.to_string())
            .collect(),
    };
    let compiled = compilers.iter().any(|compiler| {
        Command::new(compiler)
            .arg("/FO")
            .arg(&res)
            .arg(&rc)
            .status()
            .is_ok_and(|status| status.success())
    });
    assert!(
        compiled,
        "no resource compiler: install llvm-rc or set BLI_KVM_RC"
    );
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

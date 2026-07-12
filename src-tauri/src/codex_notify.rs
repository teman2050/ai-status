//! Manage AI STATUS's `notify` entry in Codex's global config (~/.codex/config.toml), so
//! packaged builds connect to Codex out of the box without Python or the repo installer
//! script (scripts/install-codex-notify.py stays for development).
//!
//! Codex has a single notify slot, so unlike Claude/Cursor hooks our entry can't just be
//! appended: an existing foreign notifier (e.g. Codex's own computer-use notification
//! program) is preserved by chaining — notify points at this app (`--hook codex`) with the
//! original command carried verbatim after `--chain`, and the hook client re-invokes it on
//! every notification. Disabling the toggle restores the original notify (or removes ours).
//!
//! The file is edited with toml_edit so comments, key order, and formatting survive. An
//! unparseable file — or a `notify` value in a shape we don't understand — is never
//! touched, and a backup is written before every change.

use std::path::{Path, PathBuf};
use toml_edit::DocumentMut;

fn config_path() -> PathBuf {
    crate::claude_hooks::home_dir().join(".codex").join("config.toml")
}

/// The app-managed notify command: `<exe> --hook codex [--chain <original notifier>...]`.
fn our_notify(exe: &Path, chain: &[String]) -> Vec<String> {
    let mut v = vec![
        exe.display().to_string(),
        "--hook".to_string(),
        "codex".to_string(),
    ];
    if !chain.is_empty() {
        v.push("--chain".to_string());
        v.extend(chain.iter().cloned());
    }
    v
}

/// Legacy = the notify ENTRY POINT is our old adapter — the installers wrote
/// `["bash", ".../asb_notify_chain.sh"]` or `["python3", ".../asb_notify.py"]`, so the
/// script sits in the first two args. A FOREIGN notifier that merely mentions our script
/// in a later argument (e.g. Codex Computer Use wrapping us via
/// `--previous-notify '["bash",".../asb_notify_chain.sh"]'`) is not ours: it must be
/// chained like any foreign notifier, never migrated (migration would drop it).
fn is_legacy(args: &[String]) -> bool {
    args.iter().take(2).any(|a| a.contains("asb_notify"))
}

/// A notify command is ours when it runs this app in hook mode, or the legacy Python
/// adapter / chain script (migrated to the app command once it manages notify itself).
fn is_ours(args: &[String]) -> bool {
    args.windows(2).any(|w| w[0] == "--hook" && w[1] == "codex") || is_legacy(args)
}

/// The original notifier carried after `--chain` in an app-managed notify command.
fn chain_of(args: &[String]) -> &[String] {
    args.iter()
        .position(|a| a == "--chain")
        .map(|i| &args[i + 1..])
        .unwrap_or(&[])
}

/// Extract `"..."`-quoted tokens (the legacy installer wrote plain quoted paths, no escapes).
fn quoted_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(start) = rest.find('"') {
        let Some(len) = rest[start + 1..].find('"') else {
            break;
        };
        out.push(rest[start + 1..start + 1 + len].to_string());
        rest = &rest[start + len + 2..];
    }
    out
}

/// Recover the user's original notifier from a legacy installer-generated chain script:
/// a `"prog" "arg" "$@" 2>/dev/null || true` line ahead of the asb_notify.py call.
fn legacy_chain_original(args: &[String]) -> Vec<String> {
    let Some(script) = args.iter().find(|a| a.contains("asb_notify_chain")) else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(script) else {
        return Vec::new();
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.contains("asb_notify") {
            continue;
        }
        if let Some(head) = line.strip_suffix("\"$@\" 2>/dev/null || true") {
            let tokens = quoted_tokens(head);
            if !tokens.is_empty() {
                return tokens;
            }
        }
    }
    Vec::new()
}

enum Change {
    None,
    Remove,
    Set(Vec<String>),
}

/// Compute the desired notify state. Foreign notifiers are chained on enable and left
/// untouched on disable; our own entry restores the chained original when disabled.
fn desired(current: Option<&[String]>, exe: &Path, enabled: bool) -> Change {
    let Some(cur) = current else {
        return if enabled {
            Change::Set(our_notify(exe, &[]))
        } else {
            Change::None
        };
    };
    if !is_ours(cur) {
        return if enabled {
            Change::Set(our_notify(exe, cur))
        } else {
            Change::None
        };
    }
    let original = if is_legacy(cur) {
        legacy_chain_original(cur)
    } else {
        chain_of(cur).to_vec()
    };
    if enabled {
        // Keep an existing app entry whose executable still exists, so a dev build and an
        // installed build don't fight over the path (same rule as claude_hooks).
        if !is_legacy(cur) && cur.first().is_some_and(|e| Path::new(e).is_file()) {
            return Change::None;
        }
        Change::Set(our_notify(exe, &original))
    } else if original.is_empty() {
        Change::Remove
    } else {
        Change::Set(original)
    }
}

/// Install (or remove) our notify entry in the given config file. Returns whether the
/// file changed. A missing file is created only when enabling; an unparseable file or an
/// unrecognized notify shape is left untouched (never clobber the user's config).
fn apply(path: &Path, exe: &Path, enabled: bool) -> std::io::Result<bool> {
    let original = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !enabled {
                return Ok(false);
            }
            String::new()
        }
        Err(e) => return Err(e),
    };
    let Ok(mut doc) = original.parse::<DocumentMut>() else {
        return Ok(false);
    };
    let current: Option<Vec<String>> = match doc.get("notify") {
        None => None,
        Some(item) => {
            let Some(arr) = item.as_array() else {
                return Ok(false);
            };
            let Some(strings) = arr
                .iter()
                .map(|v| v.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()
            else {
                return Ok(false);
            };
            Some(strings)
        }
    };

    match desired(current.as_deref(), exe, enabled) {
        Change::None => return Ok(false),
        Change::Remove => {
            doc.remove("notify");
        }
        Change::Set(args) => {
            let mut arr = toml_edit::Array::new();
            for a in &args {
                arr.push(a.as_str());
            }
            doc["notify"] = toml_edit::value(arr);
        }
    }

    let updated = doc.to_string();
    if updated == original {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let backup = path.with_file_name(format!(
            "config.toml.bak-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        ));
        let _ = std::fs::copy(path, backup);
    }
    std::fs::write(path, updated)?;
    Ok(true)
}

/// Sync the notify installation with the config toggle. Errors are logged, never fatal:
/// the board still tracks Codex via the rollout watcher without notify.
pub fn sync(enabled: bool) {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let path = config_path();
    match apply(&path, &exe, enabled) {
        Ok(true) => eprintln!(
            "[ai-status] codex notify {} in {}",
            if enabled { "installed" } else { "removed" },
            path.display()
        ),
        Ok(false) => {}
        Err(e) => eprintln!("[ai-status] codex notify sync failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            let mut p = std::env::temp_dir();
            p.push(format!("asb_codex_{tag}_{}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            fs::create_dir_all(&p).unwrap();
            TempDir(p)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn fake_exe(dir: &TempDir) -> PathBuf {
        let exe = dir.path("AI STATUS.exe");
        fs::write(&exe, "x").unwrap();
        exe
    }

    fn read_notify(path: &Path) -> Option<Vec<String>> {
        let doc = fs::read_to_string(path)
            .unwrap()
            .parse::<DocumentMut>()
            .unwrap();
        Some(
            doc.get("notify")?
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect(),
        )
    }

    #[test]
    fn install_into_missing_config_creates_notify() {
        let dir = TempDir::new("create");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        assert!(apply(&cfg, &exe, true).unwrap());
        let notify = read_notify(&cfg).unwrap();
        assert_eq!(notify[1..], ["--hook", "codex"]);
        assert_eq!(notify[0], exe.display().to_string());
    }

    #[test]
    fn foreign_notifier_is_chained_and_restored() {
        let dir = TempDir::new("chain");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        fs::write(
            &cfg,
            "# my codex config\nmodel = \"o4\"\nnotify = [\"C:/OpenAI/codex-computer-use.exe\", \"turn-ended\"]\n\n[tui]\ntheme = \"dark\"\n",
        )
        .unwrap();

        assert!(apply(&cfg, &exe, true).unwrap());
        let notify = read_notify(&cfg).unwrap();
        assert_eq!(
            notify[1..],
            [
                "--hook",
                "codex",
                "--chain",
                "C:/OpenAI/codex-computer-use.exe",
                "turn-ended"
            ]
        );
        // comments, other keys, and sections survive the lossless edit
        let text = fs::read_to_string(&cfg).unwrap();
        assert!(text.contains("# my codex config"));
        assert!(text.contains("model = \"o4\""));
        assert!(text.contains("[tui]"));

        // disabling restores the original notifier exactly
        assert!(apply(&cfg, &exe, false).unwrap());
        assert_eq!(
            read_notify(&cfg).unwrap(),
            ["C:/OpenAI/codex-computer-use.exe", "turn-ended"]
        );
    }

    /// Real-world macOS shape: Codex Computer Use is the entry point and carries OUR old
    /// chain script inside its --previous-notify JSON argument. That mention must not make
    /// it "legacy": it's a foreign notifier and gets chained intact, never migrated away.
    #[test]
    fn foreign_wrapper_mentioning_legacy_script_is_chained_not_migrated() {
        let dir = TempDir::new("wrapper");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        fs::write(
            &cfg,
            "notify = [\"/Apps/SkyComputerUseClient\", \"turn-ended\", \"--previous-notify\", '[\"bash\",\"/repo/adapters/codex/asb_notify_chain.sh\"]']\n",
        )
        .unwrap();

        assert!(apply(&cfg, &exe, true).unwrap());
        let notify = read_notify(&cfg).unwrap();
        assert_eq!(
            notify[1..],
            [
                "--hook",
                "codex",
                "--chain",
                "/Apps/SkyComputerUseClient",
                "turn-ended",
                "--previous-notify",
                r#"["bash","/repo/adapters/codex/asb_notify_chain.sh"]"#
            ],
            "the wrapper must survive verbatim in the chain"
        );

        // disabling restores the wrapper exactly as it was
        assert!(apply(&cfg, &exe, false).unwrap());
        assert_eq!(
            read_notify(&cfg).unwrap(),
            [
                "/Apps/SkyComputerUseClient",
                "turn-ended",
                "--previous-notify",
                r#"["bash","/repo/adapters/codex/asb_notify_chain.sh"]"#
            ]
        );
    }

    #[test]
    fn install_is_idempotent() {
        let dir = TempDir::new("idem");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        assert!(apply(&cfg, &exe, true).unwrap());
        assert!(!apply(&cfg, &exe, true).unwrap(), "second run must not rewrite");
    }

    #[test]
    fn uninstall_without_chain_removes_notify() {
        let dir = TempDir::new("remove");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        fs::write(&cfg, "model = \"o4\"\n").unwrap();
        apply(&cfg, &exe, true).unwrap();
        assert!(apply(&cfg, &exe, false).unwrap());
        assert!(read_notify(&cfg).is_none());
        assert!(fs::read_to_string(&cfg).unwrap().contains("model = \"o4\""));
    }

    #[test]
    fn foreign_notifier_is_untouched_when_disabled() {
        let dir = TempDir::new("foreign_off");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        fs::write(&cfg, "notify = [\"notify-send\", \"Codex\"]\n").unwrap();
        assert!(!apply(&cfg, &exe, false).unwrap());
        assert_eq!(read_notify(&cfg).unwrap(), ["notify-send", "Codex"]);
    }

    #[test]
    fn legacy_python_adapter_is_migrated() {
        let dir = TempDir::new("legacy");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        fs::write(
            &cfg,
            "notify = [\"python3\", \"/repo/adapters/codex/asb_notify.py\"]\n",
        )
        .unwrap();
        assert!(apply(&cfg, &exe, true).unwrap());
        let notify = read_notify(&cfg).unwrap();
        assert_eq!(notify[1..], ["--hook", "codex"]);
    }

    #[test]
    fn legacy_chain_script_original_is_recovered() {
        let dir = TempDir::new("legacy_chain");
        let exe = fake_exe(&dir);
        let script = dir.path("asb_notify_chain.sh");
        fs::write(
            &script,
            "#!/usr/bin/env bash\n# chained notify\n\"D:/orig/notifier.exe\" \"turn-ended\" \"$@\" 2>/dev/null || true\npython3 \"/repo/adapters/codex/asb_notify.py\" \"$@\" 2>/dev/null || true\nexit 0\n",
        )
        .unwrap();
        let cfg = dir.path("config.toml");
        fs::write(
            &cfg,
            format!("notify = [\"bash\", \"{}\"]\n", script.display().to_string().replace('\\', "/")),
        )
        .unwrap();

        assert!(apply(&cfg, &exe, true).unwrap());
        let notify = read_notify(&cfg).unwrap();
        assert_eq!(
            notify[1..],
            ["--hook", "codex", "--chain", "D:/orig/notifier.exe", "turn-ended"]
        );
        // disabling hands the slot back to the recovered original notifier
        assert!(apply(&cfg, &exe, false).unwrap());
        assert_eq!(read_notify(&cfg).unwrap(), ["D:/orig/notifier.exe", "turn-ended"]);
    }

    #[test]
    fn existing_entry_with_live_exe_is_kept_even_if_path_differs() {
        let dir = TempDir::new("keep");
        let exe_a = fake_exe(&dir);
        let exe_b = dir.path("AI STATUS 2.exe");
        fs::write(&exe_b, "x").unwrap();
        let cfg = dir.path("config.toml");
        fs::write(&cfg, "notify = [\"orig.exe\", \"turn-ended\"]\n").unwrap();
        apply(&cfg, &exe_a, true).unwrap();
        assert!(!apply(&cfg, &exe_b, true).unwrap(), "live exe_a entry stays put");
        fs::remove_file(&exe_a).unwrap();
        assert!(apply(&cfg, &exe_b, true).unwrap(), "dead exe is repaired");
        let notify = read_notify(&cfg).unwrap();
        assert_eq!(notify[0], exe_b.display().to_string());
        assert_eq!(notify[3..], ["--chain", "orig.exe", "turn-ended"], "chain survives repair");
    }

    #[test]
    fn unparseable_config_is_left_untouched() {
        let dir = TempDir::new("badtoml");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        fs::write(&cfg, "notify = [broken\n").unwrap();
        assert!(!apply(&cfg, &exe, true).unwrap());
        assert_eq!(fs::read_to_string(&cfg).unwrap(), "notify = [broken\n");
    }

    #[test]
    fn unrecognized_notify_shape_is_left_untouched() {
        let dir = TempDir::new("shape");
        let exe = fake_exe(&dir);
        let cfg = dir.path("config.toml");
        fs::write(&cfg, "notify = \"a-plain-string\"\n").unwrap();
        assert!(!apply(&cfg, &exe, true).unwrap());
        assert_eq!(
            fs::read_to_string(&cfg).unwrap(),
            "notify = \"a-plain-string\"\n"
        );
    }
}

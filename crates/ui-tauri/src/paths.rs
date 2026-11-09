//! 壳要在两种布局下找到两个东西：daemon 可执行文件与前端产物。
//!
//! 打包态：安装目录里 `openape-desktop.exe` 与 `openaped.exe`、`ui/` 并排。
//! 开发态：`target/debug/` 里只有 exe，前端在仓库根的 `ui/dist`，所以往上找。
//! 两条都不改配置——配置项留给"用户想指别处"的情形（环境变量，见 `main.rs`）。

use std::path::{Path, PathBuf};

/// 往上找几层。开发态是 `crates/ui-tauri/target/debug` 这种深度，四层够。
const MAX_UP: usize = 4;

/// daemon 可执行文件名（带平台后缀）。
pub fn daemon_file_name() -> String {
    format!("openaped{}", std::env::consts::EXE_SUFFIX)
}

/// 从 `exe_dir` 找 daemon：并排优先，其次开发态的 `target/{debug,release}`。
pub fn find_daemon(exe_dir: &Path) -> Option<PathBuf> {
    let mut dir = Some(exe_dir);
    let mut up = 0;
    while let Some(current) = dir {
        let beside = current.join(daemon_file_name());
        if beside.is_file() {
            return Some(beside);
        }
        if up < MAX_UP {
            let target = current.join("target");
            for profile in ["debug", "release"] {
                let candidate = target.join(profile).join(daemon_file_name());
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
        dir = current.parent();
        up += 1;
    }
    None
}

/// 某一层里的前端产物：`ui/` 或 `ui/dist/`，认含 `index.html` 的那层。
fn ui_here(base: &Path) -> Option<PathBuf> {
    [base.join("ui"), base.join("ui").join("dist")]
        .into_iter()
        .find(|candidate| candidate.join("index.html").is_file())
}

/// 从 `exe_dir` 找前端产物目录（含 `index.html` 的那一层）。
///
/// 往上找时只认仓库根（含 `Cargo.toml` 的那层），免得一路走到
/// `C:/` 或临时目录，撞上一个碰巧叫 `ui/index.html` 的东西。
pub fn find_ui_dir(exe_dir: &Path) -> Option<PathBuf> {
    if let Some(found) = ui_here(exe_dir) {
        return Some(found);
    }
    let mut dir = exe_dir.parent();
    let mut up = 1;
    while let Some(current) = dir {
        if up > MAX_UP {
            return None;
        }
        if current.join("Cargo.toml").is_file()
            && let Some(found) = ui_here(current)
        {
            return Some(found);
        }
        dir = current.parent();
        up += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// 每个用例一个独立目录，跑完就删。
    struct Sandbox(PathBuf);

    impl Sandbox {
        fn new(name: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("openape-shell-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("建沙箱");
            Self(dir)
        }

        fn touch(&self, relative: &str) -> PathBuf {
            let path = self.0.join(relative);
            fs::create_dir_all(path.parent().expect("有父目录")).expect("建父目录");
            fs::write(&path, b"").expect("写文件");
            path
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_packaged_layout_finds_both_beside_the_exe() {
        let sandbox = Sandbox::new("packaged");
        let exe = sandbox.touch(&format!(
            "bin/openape-desktop{}",
            std::env::consts::EXE_SUFFIX
        ));
        let daemon = sandbox.touch(&format!("bin/{}", daemon_file_name()));
        sandbox.touch("bin/ui/index.html");

        let exe_dir = exe.parent().expect("有父目录");
        assert_eq!(find_daemon(exe_dir), Some(daemon));
        assert_eq!(
            find_ui_dir(exe_dir),
            Some(sandbox.path().join("bin").join("ui"))
        );
    }

    #[test]
    fn a_dev_layout_walks_up_to_target_and_ui_dist() {
        let sandbox = Sandbox::new("dev");
        sandbox.touch("Cargo.toml");
        let exe = sandbox.touch(&format!(
            "target/debug/openape-desktop{}",
            std::env::consts::EXE_SUFFIX
        ));
        let daemon = sandbox.touch(&format!("target/debug/{}", daemon_file_name()));
        let ui = sandbox.touch("ui/dist/index.html");

        let exe_dir = exe.parent().expect("有父目录");
        assert_eq!(find_daemon(exe_dir), Some(daemon));
        assert_eq!(
            find_ui_dir(exe_dir),
            Some(ui.parent().expect("有父目录").to_path_buf())
        );
    }

    #[test]
    fn nothing_found_is_none_not_a_guess() {
        let sandbox = Sandbox::new("empty");
        let exe = sandbox.touch(&format!("openape-desktop{}", std::env::consts::EXE_SUFFIX));
        let exe_dir = exe.parent().expect("有父目录");
        assert_eq!(find_daemon(exe_dir), None);
        assert_eq!(find_ui_dir(exe_dir), None);
    }
}

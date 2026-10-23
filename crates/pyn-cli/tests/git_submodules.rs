//! Gitlink status and diff compared with real git; lifecycle operations only build the fixtures.

#[path = "common/gitfix.rs"]
mod gitfix;
#[path = "../../pyn-gittransport/tests/common/gitsrv.rs"]
mod gitsrv;

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use gitsrv::*;
use tempfile::{TempDir, tempdir};

struct Fixture {
    _tmp: TempDir,
    repo: PathBuf,
    sub: PathBuf,
    second: String,
    config: String,
}

impl Fixture {
    fn new(format: &str, gitfile: bool) -> Self {
        let tmp = tempdir().unwrap();
        let repo = tmp.path().canonicalize().unwrap().join("repo");
        let sub = repo.join("sub");
        init(&repo, format, false);
        init(&sub, format, false);
        let first = commit(&sub, "file", b"one\n", "one");
        let second = commit(&sub, "file", b"two\n", "two");
        run(&sub, &["checkout", "-q", &first]);
        fs::write(repo.join(".gitmodules"), Self::modules(None)).unwrap();
        run(&repo, &["add", ".gitmodules"]);
        run(
            &repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{first},sub"),
            ],
        );
        run(&repo, &["commit", "-qm", "base"]);
        if gitfile {
            fs::create_dir_all(repo.join(".git/modules")).unwrap();
            fs::rename(sub.join(".git"), repo.join(".git/modules/library")).unwrap();
            fs::write(sub.join(".git"), "gitdir: ../.git/modules/library\n").unwrap();
        }
        let config = fs::read_to_string(repo.join(".git/config")).unwrap();
        Self {
            _tmp: tmp,
            repo,
            sub,
            second,
            config,
        }
    }

    fn modules(ignore: Option<&str>) -> String {
        format!(
            "[submodule \"library\"]\n path = sub\n url = ../source\n{}",
            ignore.map_or(String::new(), |v| format!(" ignore = {v}\n"))
        )
    }

    fn configure(&self, default: Option<&str>, local: Option<&str>, modules: Option<&str>) {
        fs::write(
            self.repo.join(".git/config"),
            format!(
                "{}{}{}",
                self.config,
                default.map_or(String::new(), |v| format!(
                    "\n[diff]\n ignoreSubmodules = {v}\n"
                )),
                local.map_or(String::new(), |v| format!(
                    "\n[submodule \"library\"]\n ignore = {v}\n"
                ))
            ),
        )
        .unwrap();
        fs::write(self.repo.join(".gitmodules"), Self::modules(modules)).unwrap();
    }

    fn state(&self, state: &str) {
        match state {
            "clean" => {}
            "forward" => {
                run(&self.sub, &["checkout", "-q", "main"]);
            }
            "backward" => {
                run(
                    &self.repo,
                    &[
                        "update-index",
                        "--cacheinfo",
                        &format!("160000,{},sub", self.second),
                    ],
                );
                run(&self.repo, &["commit", "-qm", "record second"]);
            }
            "detached" => {
                run(&self.sub, &["checkout", "-q", "--detach", &self.second]);
            }
            "tracked" => {
                fs::write(self.sub.join("file"), "dirty\n").unwrap();
            }
            "index" => {
                fs::write(self.sub.join("file"), "staged\n").unwrap();
                run(&self.sub, &["add", "file"]);
            }
            "untracked" => {
                fs::write(self.sub.join("new"), "new\n").unwrap();
            }
            "staged" => {
                run(&self.sub, &["checkout", "-q", "main"]);
                run(
                    &self.repo,
                    &[
                        "update-index",
                        "--cacheinfo",
                        &format!("160000,{},sub", self.second),
                    ],
                );
            }
            "missing" => {
                fs::rename(&self.sub, self.repo.parent().unwrap().join("saved")).unwrap();
            }
            "uninitialized" => {
                fs::rename(
                    self.sub.join(".git"),
                    self.repo.parent().unwrap().join("saved-git"),
                )
                .unwrap();
                fs::remove_file(self.sub.join("file")).unwrap();
            }
            other => panic!("unknown state {other}"),
        }
    }

    fn pyn(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_pyn"))
            .current_dir(&self.repo)
            .args(args)
            .env_clear()
            .env("PATH", path_env())
            .env("HOME", self.repo.parent().unwrap().join("home"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("TZ", "UTC")
            .output()
            .unwrap()
    }

    fn same(&self, args: &[&str]) {
        self.compare(args, 0);
    }

    fn compare(&self, args: &[&str], code: i32) {
        // Neither inspecting the submodule nor the oracle may refresh its index.
        let before = gitfix::snapshot(&self.sub_git_dir());
        let worktree = self.sub.is_dir().then(|| gitfix::snapshot(&self.sub));
        let want = git(&self.repo)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("TZ", "UTC")
            .args(args)
            .output()
            .unwrap();
        let got = self.pyn(args);
        assert_eq!(
            want.status.code(),
            Some(code),
            "oracle failed: {}",
            String::from_utf8_lossy(&want.stderr)
        );
        assert_eq!(
            got.status.code(),
            want.status.code(),
            "{args:?}: {}",
            String::from_utf8_lossy(&got.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&got.stdout),
            String::from_utf8_lossy(&want.stdout),
            "{args:?}"
        );
        assert_eq!(
            gitfix::snapshot(&self.sub_git_dir()),
            before,
            "inspection wrote to submodule git directory"
        );
        assert_eq!(
            self.sub.is_dir().then(|| gitfix::snapshot(&self.sub)),
            worktree,
            "inspection wrote to submodule worktree"
        );
    }

    fn sub_git_dir(&self) -> PathBuf {
        let dir = self.repo.join(".git/modules/library");
        if dir.is_dir() {
            dir
        } else if self.sub.join(".git").is_dir() {
            self.sub.join(".git")
        } else if self.repo.parent().unwrap().join("saved/.git").is_dir() {
            self.repo.parent().unwrap().join("saved/.git")
        } else {
            self.repo.parent().unwrap().join("saved-git")
        }
    }

    fn table(&self) {
        let before = gitfix::snapshot(&self.sub_git_dir());
        let worktree = self.sub.is_dir().then(|| gitfix::snapshot(&self.sub));
        let want = git(&self.repo)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .args(["status", "--porcelain", "--", "sub"])
            .output()
            .unwrap();
        assert!(
            want.status.success(),
            "{}",
            String::from_utf8_lossy(&want.stderr)
        );
        let want = String::from_utf8(want.stdout).unwrap();
        let got = self.pyn(&["status", "sub"]);
        assert!(
            got.status.success(),
            "{}",
            String::from_utf8_lossy(&got.stderr)
        );
        assert_eq!(
            gitfix::porcelain(&String::from_utf8_lossy(&got.stdout)),
            gitfix::sorted_lines(&want)
        );
        assert_eq!(gitfix::snapshot(&self.sub_git_dir()), before);
        assert_eq!(
            self.sub.is_dir().then(|| gitfix::snapshot(&self.sub)),
            worktree
        );
    }
}

const STATES: [&str; 10] = [
    "clean",
    "forward",
    "backward",
    "detached",
    "tracked",
    "index",
    "untracked",
    "staged",
    "missing",
    "uninitialized",
];

fn each(check: impl Fn(&Fixture)) {
    for format in FORMATS {
        for gitfile in [false, true] {
            check(&Fixture::new(format, gitfile));
        }
    }
}

#[test]
fn short_status_gitlinks() {
    for state in STATES {
        each(|f| {
            f.state(state);
            f.same(&["status", "-s", "--", "sub"]);
        });
    }
}

#[test]
fn porcelain_status_gitlinks() {
    for state in STATES {
        each(|f| {
            f.state(state);
            f.same(&["status", "--porcelain", "--", "sub"]);
        });
    }
}

#[test]
fn default_status_gitlinks() {
    for state in STATES {
        each(|f| {
            f.state(state);
            f.table();
        });
    }
}

#[test]
fn worktree_diff_gitlinks() {
    for state in STATES {
        each(|f| {
            f.state(state);
            f.same(&["diff", "--", "sub"]);
        });
    }
}

#[test]
fn revision_worktree_diff_gitlinks() {
    for state in STATES {
        each(|f| {
            f.state(state);
            f.same(&["diff", "HEAD", "--", "sub"]);
        });
    }
}

#[test]
fn cached_diff_gitlinks() {
    each(|f| {
        f.state("staged");
        f.same(&["diff", "--cached"]);
        f.same(&["diff", "--staged", "HEAD"]);
    });
}

fn record_move(f: &Fixture) {
    f.state("staged");
    run(&f.repo, &["commit", "-qm", "moved"]);
}

#[test]
fn tree_diff_gitlinks() {
    each(|f| {
        record_move(f);
        f.same(&["diff", "HEAD~1", "HEAD"]);
    });
}

#[test]
fn show_gitlinks() {
    each(|f| {
        record_move(f);
        f.same(&["show", "--format=oneline"]);
        f.configure(None, Some("all"), None);
        f.same(&["show", "--format=oneline"]);
    });
}

#[test]
fn log_patch_gitlinks() {
    each(|f| {
        record_move(f);
        f.same(&["log", "-n", "1", "--format=oneline", "-p"]);
        f.configure(None, Some("all"), None);
        f.same(&["log", "-n", "1", "--format=oneline", "-p"]);
    });
}

#[test]
fn dirty_submodule_short_status() {
    each(|f| {
        f.state("tracked");
        f.state("untracked");
        f.same(&["status", "-s"]);
    });
}

#[test]
fn untracked_submodule_short_status() {
    each(|f| {
        f.state("untracked");
        f.same(&["status", "-s"]);
    });
}

#[test]
fn dirty_gitlink_diff() {
    each(|f| {
        f.state("tracked");
        f.same(&["diff"]);
        run(&f.sub, &["checkout", "--", "file"]);
        f.state("detached");
        f.state("tracked");
        f.same(&["diff"]);
    });
}

#[test]
fn untracked_gitlink_diff_config() {
    each(|f| {
        f.state("untracked");
        f.configure(None, Some("none"), None);
        f.same(&["diff"]);
    });
}

#[test]
fn ignore_configuration_status() {
    for state in ["tracked", "index", "untracked", "forward"] {
        each(|f| {
            f.state(state);
            for default in [None, Some("all"), Some("dirty")] {
                for local in [
                    None,
                    Some("none"),
                    Some("untracked"),
                    Some("dirty"),
                    Some("all"),
                ] {
                    f.configure(default, local, None);
                    f.same(&["status", "-s", "--", "sub"]);
                    f.same(&["status", "--porcelain", "--", "sub"]);
                    f.table();
                }
            }
        });
    }
}

#[test]
fn ignore_configuration_diff() {
    for state in ["tracked", "index", "untracked", "forward"] {
        each(|f| {
            f.state(state);
            for default in [None, Some("all"), Some("dirty")] {
                for local in [
                    None,
                    Some("none"),
                    Some("untracked"),
                    Some("dirty"),
                    Some("all"),
                ] {
                    f.configure(default, local, None);
                    f.same(&["diff", "--", "sub"]);
                    f.same(&["diff", "HEAD", "--", "sub"]);
                }
            }
        });
    }
}

#[test]
fn gitmodules_and_config_precedence() {
    for state in ["tracked", "untracked", "forward"] {
        each(|f| {
            f.state(state);
            for modules in ["none", "untracked", "dirty", "all"] {
                for local in [
                    None,
                    Some("none"),
                    Some("untracked"),
                    Some("dirty"),
                    Some("all"),
                ] {
                    f.configure(Some("all"), local, Some(modules));
                    f.same(&["status", "-s", "--", "sub"]);
                    f.table();
                    f.same(&["diff", "--", "sub"]);
                }
            }
        });
    }
}

#[test]
fn staged_gitlinks_under_ignore_all() {
    each(|f| {
        f.state("staged");
        for (default, local) in [
            (Some("all"), None),
            (None, Some("all")),
            (Some("all"), Some("none")),
        ] {
            f.configure(default, local, None);
            f.same(&["status", "-s", "--", "sub"]);
            f.table();
            f.same(&["diff", "--cached", "--", "sub"]);
        }
    });
}

#[test]
fn deleted_gitlinks_diff() {
    each(|f| {
        run(&f.repo, &["update-index", "--force-remove", "sub"]);
        f.same(&["diff", "--cached", "--", "sub"]);
    });
}

#[test]
fn added_gitlinks_diff() {
    each(|f| {
        run(&f.repo, &["update-index", "--force-remove", "sub"]);
        run(&f.repo, &["commit", "-qm", "remove gitlink"]);
        run(
            &f.repo,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{},sub", f.second),
            ],
        );
        f.same(&["diff", "--cached", "--", "sub"]);
    });
}

#[test]
fn malformed_submodule_gitfile() {
    for format in FORMATS {
        let f = Fixture::new(format, true);
        fs::write(f.sub.join(".git"), "invalid gitfile\n").unwrap();
        f.compare(&["status", "-s"], 128);
        f.compare(&["diff"], 128);
    }
}

#[test]
fn stash_show_gitlinks() {
    each(|f| {
        f.state("staged");
        let base = run(&f.repo, &["rev-parse", "HEAD"]);
        let tree = run(&f.repo, &["write-tree"]);
        let index = run(
            &f.repo,
            &["commit-tree", tree.trim(), "-p", base.trim(), "-m", "index"],
        );
        let untracked = run(&f.repo, &["commit-tree", tree.trim(), "-m", "untracked"]);
        let stash = run(
            &f.repo,
            &[
                "commit-tree",
                tree.trim(),
                "-p",
                base.trim(),
                "-p",
                index.trim(),
                "-p",
                untracked.trim(),
                "-m",
                "stash",
            ],
        );
        run(
            &f.repo,
            &[
                "update-ref",
                "--create-reflog",
                "-m",
                "stash",
                "refs/stash",
                stash.trim(),
            ],
        );
        f.same(&["stash", "show", "-p"]);
        f.same(&["stash", "show", "--only-untracked", "-p"]);
        f.configure(None, Some("all"), None);
        f.same(&["stash", "show", "-p"]);
        f.same(&["stash", "show", "--only-untracked", "-p"]);
    });
}

#[test]
fn ignore_all_gitlink_typechange() {
    each(|f| {
        let id = run(&f.repo, &["rev-parse", "HEAD:.gitmodules"]);
        run(
            &f.repo,
            &[
                "update-index",
                "--cacheinfo",
                &format!("100644,{},sub", id.trim()),
            ],
        );
        f.configure(None, Some("all"), None);
        f.same(&["diff", "--cached", "--", "sub"]);
        f.same(&["diff", "--cached", "--name-status", "--", "sub"]);
    });
}

#[test]
fn merge_stat_gitlinks() {
    for ignore in [None, Some("all")] {
        each(|f| {
            f.configure(None, ignore, None);
            record_move(f);
            run(&f.repo, &["branch", "side"]);
            run(&f.repo, &["reset", "--hard", "HEAD~1"]);
            f.state("uninitialized");
            let oracle = f.repo.parent().unwrap().join("oracle");
            gitfix::copy_tree(&f.repo, &oracle);
            let want = run(&oracle, &["merge", "side"]);
            let got = f.pyn(&["merge", "side"]);
            assert!(
                got.status.success(),
                "{}",
                String::from_utf8_lossy(&got.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&got.stdout), want);
        });
    }
}

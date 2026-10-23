//! Read-only gitlink inspection and the superproject's submodule ignore settings.

use pyn_git::Config;
use pyn_object::ObjectId;

use crate::error::{Error, Result, io_at};
use crate::{Kind, Repo, fs_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmoduleIgnore {
    None,
    Untracked,
    Dirty,
    All,
}

/// The visible changes within an initialized submodule, after ignore settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubmoduleStatus {
    pub head: ObjectId,
    pub new_commits: bool,
    pub modified: bool,
    pub untracked: bool,
}

impl SubmoduleStatus {
    pub fn changed(&self) -> bool {
        self.new_commits || self.modified || self.untracked
    }
}

impl Repo {
    /// Per-name config overrides `.gitmodules`, then `diff.ignoreSubmodules`, then the command's default.
    pub fn submodule_ignore(
        &self,
        path: &[u8],
        default: SubmoduleIgnore,
    ) -> Result<SubmoduleIgnore> {
        let file = self.root()?.join(".gitmodules");
        let modules = match std::fs::symlink_metadata(&file) {
            Ok(m) if m.is_file() => Config::read(&file)?,
            Ok(_) => Config::default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
            Err(e) => return Err(io_at(&file)(e)),
        };
        let name = modules.entries().iter().rev().find_map(|e| {
            (e.section == "submodule"
                && e.key == "path"
                && e.value.as_deref().map(str::as_bytes) == Some(path))
            .then_some(e.subsection.as_deref())
            .flatten()
        });
        let per_name = name.and_then(|name| {
            let key = format!("submodule.{name}.ignore");
            self.config().get(&key).or_else(|| modules.get(&key))
        });
        let value = per_name.or_else(|| self.config().get("diff.ignoresubmodules"));
        Ok(match value {
            None => default,
            Some("none") => SubmoduleIgnore::None,
            Some("untracked") => SubmoduleIgnore::Untracked,
            Some("dirty") => SubmoduleIgnore::Dirty,
            Some("all") => SubmoduleIgnore::All,
            Some(v) => {
                return Err(Error::Invalid(format!(
                    "bad --ignore-submodules argument: {v}"
                )));
            }
        })
    }

    /// A missing `.git` is an uninitialized checkout; opening never discovers the parent repository.
    pub fn submodule_status(
        &self,
        path: &[u8],
        recorded: ObjectId,
        default: SubmoduleIgnore,
    ) -> Result<Option<SubmoduleStatus>> {
        let ignore = self.submodule_ignore(path, default)?;
        if ignore == SubmoduleIgnore::All {
            return Ok(None);
        }
        let root = fs_path(self.root()?, path);
        if !root.join(".git").exists() {
            return Ok(None);
        }
        let sub = self.open_submodule(&root)?;
        let Some(head) = sub.head_commit()? else {
            return Ok(None);
        };
        let mut state = SubmoduleStatus {
            head,
            new_commits: head != recorded,
            modified: false,
            untracked: false,
        };
        if ignore != SubmoduleIgnore::Dirty {
            for e in sub.status()?.entries {
                state.modified |= e.staged.is_some()
                    || match e.submodule {
                        Some(nested) => nested.new_commits || nested.modified,
                        None => e.unstaged.is_some_and(|k| k != Kind::Untracked),
                    };
                state.untracked |=
                    e.unstaged == Some(Kind::Untracked) || e.submodule.is_some_and(|s| s.untracked);
            }
            state.untracked &= ignore != SubmoduleIgnore::Untracked;
        }
        Ok(Some(state))
    }
}

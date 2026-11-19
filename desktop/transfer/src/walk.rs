//! Parallel folder walk. Every subfolder is read on its own thread (8 at once, 16 for network
//! paths), which matters most on network shares, where reading each folder's listing is a
//! round trip to the file server and a one-at-a-time walk spends most of its time waiting.
//!
//! Same rules as a plain walk: symbolic links (and Windows junctions) are not followed, and
//! folders or entries that can't be read are counted, not fatal.

use std::fs::DirEntry;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use rayon::{Scope, ThreadPool, ThreadPoolBuilder};

/// What the walk reports to `visit`.
pub enum Item<'a> {
    /// A regular file. `rel` is its path below the root with `/` separators; `depth` is 1 for
    /// files directly in the root.
    File { entry: &'a DirEntry, rel: &'a str, depth: usize },
    /// A folder below the root (the root itself isn't reported).
    Dir { rel: &'a str, depth: usize },
    /// A folder or entry that couldn't be read.
    Error,
}

const LOCAL_THREADS: usize = 8;
const NETWORK_THREADS: usize = 16;

fn pool(network: bool) -> &'static ThreadPool {
    static LOCAL: OnceLock<ThreadPool> = OnceLock::new();
    static NETWORK: OnceLock<ThreadPool> = OnceLock::new();
    let (cell, threads, name) = if network { (&NETWORK, NETWORK_THREADS, "net") } else { (&LOCAL, LOCAL_THREADS, "local") };
    cell.get_or_init(|| {
        ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(move |i| format!("jetstream-walk-{name}-{i}"))
            .build()
            .expect("folder walk threads")
    })
}

/// A UNC path (\\server\share) or a //server/share path: a network share.
pub fn is_network_path(path: &Path) -> bool {
    let text = path.as_os_str().to_string_lossy();
    (text.starts_with(r"\\") && !text.starts_with(r"\\?\") && !text.starts_with(r"\\.\")) || text.starts_with(r"\\?\UNC\") || text.starts_with("//")
}

/// Walk `root` (only its own entries unless `recursive`). Each walk thread keeps its own
/// accumulator, made by `init` and updated by `visit`; the accumulators are returned (merge
/// them). Order is not defined. Stops reading new folders once `cancel` is set.
pub fn par_walk<A, I, V>(root: &Path, recursive: bool, cancel: Option<&AtomicBool>, init: I, visit: V) -> Vec<A>
where
    A: Send,
    I: Fn() -> A + Sync,
    V: Fn(&mut A, Item) + Sync,
{
    let pool = pool(is_network_path(root));
    let slots: Vec<Mutex<Option<A>>> = (0..pool.current_num_threads()).map(|_| Mutex::new(None)).collect();
    let walker = Walker { slots: &slots, init: &init, visit: &visit, cancel, recursive };
    pool.scope(|scope| walker.read_dir(scope, root.to_path_buf(), String::new(), 1));
    slots.into_iter().filter_map(|slot| slot.into_inner().unwrap_or_else(|e| e.into_inner())).collect()
}

struct Walker<'w, A, I, V> {
    slots: &'w [Mutex<Option<A>>],
    init: &'w I,
    visit: &'w V,
    cancel: Option<&'w AtomicBool>,
    recursive: bool,
}

impl<'w, A, I, V> Walker<'w, A, I, V>
where
    A: Send,
    I: Fn() -> A + Sync,
    V: Fn(&mut A, Item) + Sync,
{
    fn report(&self, item: Item) {
        let index = rayon::current_thread_index().unwrap_or(0) % self.slots.len();
        let mut slot = self.slots[index].lock().unwrap_or_else(|e| e.into_inner());
        (self.visit)(slot.get_or_insert_with(self.init), item);
    }

    /// Read one folder (files at `depth`), handing each subfolder to another thread.
    fn read_dir<'s>(&'s self, scope: &Scope<'s>, dir: PathBuf, rel: String, depth: usize)
    where
        'w: 's,
    {
        if self.cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => return self.report(Item::Error),
        };
        for entry in entries {
            let Ok(entry) = entry else {
                self.report(Item::Error);
                continue;
            };
            let Ok(file_type) = entry.file_type() else {
                self.report(Item::Error);
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child_rel = if rel.is_empty() { name.into_owned() } else { format!("{rel}/{name}") };
            if file_type.is_file() {
                self.report(Item::File { entry: &entry, rel: &child_rel, depth });
            } else if file_type.is_dir() {
                self.report(Item::Dir { rel: &child_rel, depth });
                if self.recursive {
                    let path = entry.path();
                    scope.spawn(move |scope| self.read_dir(scope, path, child_rel, depth + 1));
                }
            }
            // Anything else (symbolic links, junctions, devices) is skipped, as before.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    fn tree() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..30 {
            for j in 0..5 {
                let sub = dir.path().join(format!("d{i}/e{j}"));
                std::fs::create_dir_all(&sub).unwrap();
                std::fs::write(sub.join("f.txt"), vec![b'x'; i + j]).unwrap();
            }
            std::fs::write(dir.path().join(format!("d{i}/top{i}.dat")), b"top").unwrap();
        }
        std::fs::write(dir.path().join("root.txt"), b"root").unwrap();
        std::fs::create_dir_all(dir.path().join("empty/deeper")).unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("d0"), dir.path().join("link_dir")).unwrap();
            std::os::unix::fs::symlink(dir.path().join("root.txt"), dir.path().join("link_file")).unwrap();
        }
        dir
    }

    /// (files, folders) the way walkdir (the old walk) sees them.
    fn walkdir_view(root: &Path, recursive: bool) -> (BTreeSet<String>, BTreeSet<String>) {
        let (mut files, mut dirs) = (BTreeSet::new(), BTreeSet::new());
        for entry in walkdir::WalkDir::new(root).follow_links(false).max_depth(if recursive { usize::MAX } else { 1 }) {
            let entry = entry.unwrap();
            let rel = entry.path().strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if entry.depth() == 0 {
                continue;
            }
            if entry.file_type().is_file() {
                files.insert(rel);
            } else if entry.file_type().is_dir() {
                dirs.insert(rel);
            }
        }
        (files, dirs)
    }

    fn par_view(root: &Path, recursive: bool) -> (BTreeSet<String>, BTreeSet<String>) {
        let parts = par_walk(
            root,
            recursive,
            None,
            || (BTreeSet::new(), BTreeSet::new()),
            |(files, dirs): &mut (BTreeSet<String>, BTreeSet<String>), item| match item {
                Item::File { entry, rel, depth } => {
                    assert_eq!(depth, rel.matches('/').count() + 1);
                    assert!(entry.path().ends_with(rel.rsplit('/').next().unwrap()));
                    files.insert(rel.to_string());
                }
                Item::Dir { rel, .. } => {
                    dirs.insert(rel.to_string());
                }
                Item::Error => panic!("unexpected error"),
            },
        );
        parts.into_iter().fold((BTreeSet::new(), BTreeSet::new()), |(mut f, mut d), (pf, pd)| {
            f.extend(pf);
            d.extend(pd);
            (f, d)
        })
    }

    #[test]
    fn finds_what_walkdir_finds() {
        let dir = tree();
        let (files, dirs) = par_view(dir.path(), true);
        assert_eq!((files.clone(), dirs.clone()), walkdir_view(dir.path(), true));
        assert_eq!(files.len(), 30 * 5 + 30 + 1, "no symlinked files, nothing under the symlinked folder");
        assert!(dirs.contains("empty/deeper"));
        assert_eq!(par_view(dir.path(), false), walkdir_view(dir.path(), false));
    }

    #[test]
    fn unreadable_folders_are_counted() {
        let parts = par_walk(Path::new("/definitely/not/here"), true, None, || 0u32, |n, item| {
            if matches!(item, Item::Error) {
                *n += 1
            }
        });
        assert_eq!(parts.into_iter().sum::<u32>(), 1);
    }

    #[test]
    fn stops_when_cancelled() {
        let dir = tree();
        let cancel = AtomicBool::new(true);
        let parts = par_walk(dir.path(), true, Some(&cancel), || 0u32, |n, _| *n += 1);
        assert_eq!(parts.into_iter().sum::<u32>(), 0);
    }

    #[test]
    fn network_paths() {
        assert!(is_network_path(Path::new(r"\\server\share\data")));
        assert!(is_network_path(Path::new(r"\\?\UNC\server\share")));
        assert!(is_network_path(Path::new("//server/share")));
        assert!(!is_network_path(Path::new(r"\\?\C:\data")));
        assert!(!is_network_path(Path::new(r"C:\data")));
        assert!(!is_network_path(Path::new("/home/user")));
    }
}

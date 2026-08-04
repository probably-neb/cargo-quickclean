use std::collections::{HashSet, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread;

const HELP: &str = "Atomically detach and remove Cargo target directories

Usage: cargo quickclean [OPTIONS] <PATHS>...

Arguments:
  <PATHS>...  Paths or globs to search for Cargo projects

Options:
  -H, --hidden           Search hidden directories
  -d, --dry              List and measure targets without removing them
  -i, --ignore <PATH>    Ignore a directory; can occur more than once
  -s, --no-size          Do not print removed sizes
  -h, --help             Print help
  -V, --version          Print version
";

struct Removal {
    path: PathBuf,
    display: PathBuf,
}

struct WorkState {
    search: VecDeque<PathBuf>,
    removals: VecDeque<Removal>,
    seen_search: HashSet<PathBuf>,
    seen_targets: HashSet<PathBuf>,
    active_search: usize,
    active_removals: usize,
    total: usize,
    failed: bool,
}

struct Shared {
    state: Mutex<WorkState>,
    wake: Condvar,
    next_stage: AtomicUsize,
    process_id: u32,
}

fn main() {
    let mut values = env::args_os().skip(1).collect::<Vec<_>>();
    if values.first().is_some_and(|value| value == "quickclean") {
        values.remove(0);
    }

    let mut hidden = false;
    let mut dry = false;
    let mut no_size = false;
    let mut ignores = Vec::new();
    let mut paths = Vec::new();
    let mut options = true;
    let mut index = 0;
    while index < values.len() {
        let value = &values[index];
        if options && value == "--" {
            options = false;
            index += 1;
            continue;
        }
        if !options {
            paths.push(PathBuf::from(value));
            index += 1;
            continue;
        }

        match value.to_str() {
            Some("-h" | "--help") => {
                print!("{HELP}");
                return;
            }
            Some("-V" | "--version") => {
                println!("cargo-quickclean {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            Some("-H" | "--hidden") => hidden = true,
            Some("-d" | "--dry") => dry = true,
            Some("-s" | "--no-size") => no_size = true,
            Some("-i" | "--ignore") => {
                index += 1;
                let Some(path) = values.get(index) else {
                    eprintln!("cargo quickclean: {value:?} requires a path");
                    process::exit(1);
                };
                ignores.push(PathBuf::from(path));
            }
            Some(text) if text.starts_with("--ignore=") => {
                ignores.push(PathBuf::from(&text[9..]));
            }
            Some(text) if text.starts_with('-') => {
                if text.len() > 2
                    && text[1..]
                        .chars()
                        .all(|flag| matches!(flag, 'H' | 'd' | 's'))
                {
                    for flag in text[1..].chars() {
                        match flag {
                            'H' => hidden = true,
                            'd' => dry = true,
                            's' => no_size = true,
                            _ => unreachable!(),
                        }
                    }
                } else {
                    eprintln!("cargo quickclean: unknown option {text}\n\n{HELP}");
                    process::exit(1);
                }
            }
            _ => paths.push(PathBuf::from(value)),
        }
        index += 1;
    }

    if paths.is_empty() {
        eprintln!("cargo quickclean: a path or glob is required\n\n{HELP}");
        process::exit(1);
    }

    let current_directory = match env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("cargo quickclean: cannot get the current directory: {error}");
            process::exit(1);
        }
    };
    let ignores = ignores
        .into_iter()
        .map(|path| absolute_path(path, &current_directory))
        .collect::<Vec<_>>();
    let mut search = VecDeque::new();
    let mut seen_search = HashSet::new();
    for path in paths {
        let path = absolute_path(path, &current_directory);
        if !path.exists() {
            eprintln!("cargo quickclean: {} does not exist", path.display());
            process::exit(1);
        }
        if seen_search.insert(path.clone()) {
            search.push_back(path);
        }
    }

    let shared = Shared {
        state: Mutex::new(WorkState {
            search,
            removals: VecDeque::new(),
            seen_search,
            seen_targets: HashSet::new(),
            active_search: 0,
            active_removals: 0,
            total: 0,
            failed: false,
        }),
        wake: Condvar::new(),
        next_stage: AtomicUsize::new(0),
        process_id: process::id(),
    };
    let lane_count = thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let reports = thread::scope(|scope| {
        (0..lane_count)
            .map(|_| scope.spawn(|| run(&shared, hidden, dry, no_size, &ignores)))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|lane| lane.join().expect("cleanup lane panicked"))
            .collect::<Vec<_>>()
    });

    let state = shared.state.into_inner().unwrap();
    let bytes = reports
        .iter()
        .fold(0_u64, |total, report| total.saturating_add(report.0));
    let complete = reports.iter().map(|report| report.1).sum::<usize>();
    if state.total == 0 {
        println!("No target directories found.");
    } else {
        let count = if complete == state.total {
            format!("{} directories", state.total)
        } else {
            format!("{complete} of {} directories", state.total)
        };
        let action = if dry { "would remove" } else { "removed" };
        if no_size {
            println!("{action} {count}");
        } else {
            println!("{action} -{} from {count}", format_size(bytes));
        }
    }

    if state.failed || complete != state.total {
        process::exit(1);
    }
}

fn run(
    shared: &Shared,
    hidden: bool,
    dry: bool,
    no_size: bool,
    ignores: &[PathBuf],
) -> (u64, usize) {
    let mut removed_bytes = 0_u64;
    let mut complete = 0;
    loop {
        let mut state = shared.state.lock().unwrap();
        while state.removals.is_empty()
            && state.search.is_empty()
            && (state.active_search != 0 || state.active_removals != 0)
        {
            state = shared.wake.wait(state).unwrap();
        }
        if let Some(removal) = state.removals.pop_front() {
            state.active_removals += 1;
            drop(state);

            enum Work {
                Visit(PathBuf),
                Directory {
                    path: PathBuf,
                    metadata: fs::Metadata,
                    entries: fs::ReadDir,
                },
            }

            let mut bytes = 0_u64;
            let mut errors = Vec::new();
            let mut stack = vec![Work::Visit(removal.path.clone())];
            while let Some(work) = stack.pop() {
                match work {
                    Work::Directory {
                        path,
                        metadata,
                        mut entries,
                    } => match entries.next() {
                        Some(Ok(entry)) => {
                            stack.push(Work::Directory {
                                path,
                                metadata,
                                entries,
                            });
                            stack.push(Work::Visit(entry.path()));
                        }
                        Some(Err(error)) => {
                            errors.push(format!(
                                "cannot read an entry in {}: {error}",
                                path.display()
                            ));
                            stack.push(Work::Directory {
                                path,
                                metadata,
                                entries,
                            });
                        }
                        None => {
                            let result = if dry { Ok(()) } else { fs::remove_dir(&path) };
                            match result {
                                Ok(()) => {
                                    bytes = bytes.saturating_add(allocated_size(&metadata));
                                }
                                Err(error) => errors
                                    .push(format!("cannot remove {}: {error}", path.display())),
                            }
                        }
                    },
                    Work::Visit(path) => {
                        let metadata = match fs::symlink_metadata(&path) {
                            Ok(metadata) => metadata,
                            Err(error) => {
                                errors.push(format!("cannot inspect {}: {error}", path.display()));
                                continue;
                            }
                        };
                        if metadata.is_dir() {
                            match fs::read_dir(&path) {
                                Ok(entries) => stack.push(Work::Directory {
                                    path,
                                    metadata,
                                    entries,
                                }),
                                Err(error) => {
                                    errors.push(format!("cannot read {}: {error}", path.display()))
                                }
                            }
                            continue;
                        }

                        let result = if dry { Ok(()) } else { fs::remove_file(&path) };
                        match result {
                            Ok(()) => bytes = bytes.saturating_add(allocated_size(&metadata)),
                            Err(error) => {
                                errors.push(format!("cannot remove {}: {error}", path.display()))
                            }
                        }
                    }
                }
            }

            let action = if errors.is_empty() {
                if dry { "would clean" } else { "cleaned" }
            } else {
                "partially cleaned"
            };
            if no_size {
                println!("{action}: {}", removal.display.display());
            } else {
                println!(
                    "{action} -{}: {}",
                    format_size(bytes),
                    removal.display.display()
                );
            }
            for error in &errors {
                eprintln!("cargo quickclean: {error}");
            }

            removed_bytes = removed_bytes.saturating_add(bytes);
            complete += usize::from(errors.is_empty());
            let mut state = shared.state.lock().unwrap();
            state.failed |= !errors.is_empty();
            state.active_removals -= 1;
            drop(state);
            shared.wake.notify_all();
            continue;
        }
        let Some(path) = state.search.pop_front() else {
            break;
        };
        state.active_search += 1;
        drop(state);

        let mut children = Vec::new();
        let mut errors = Vec::new();
        let mut target = None;

        'search: {
            if ignores.iter().any(|ignore| path.starts_with(ignore)) {
                break 'search;
            }
            let metadata = match fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    errors.push(format!("cannot inspect {}: {error}", path.display()));
                    break 'search;
                }
            };
            if metadata.file_type().is_symlink() {
                break 'search;
            }
            if metadata.is_file() {
                if path.file_name() == Some(OsStr::new("Cargo.toml")) {
                    let candidate = path.parent().unwrap_or(Path::new(".")).join("target");
                    if fs::symlink_metadata(&candidate).is_ok_and(|metadata| {
                        metadata.is_dir() || metadata.file_type().is_symlink()
                    }) {
                        target = Some((candidate, false));
                    }
                }
                break 'search;
            }
            if !metadata.is_dir() {
                break 'search;
            }
            if is_stale(&path) {
                target = Some((path.clone(), true));
                break 'search;
            }
            if path.file_name() == Some(OsStr::new("target"))
                && path
                    .parent()
                    .is_some_and(|parent| parent.join("Cargo.toml").is_file())
            {
                target = Some((path.clone(), false));
                break 'search;
            }

            let is_project = path.join("Cargo.toml").is_file();
            if is_project {
                let candidate = path.join("target");
                if fs::symlink_metadata(&candidate)
                    .is_ok_and(|metadata| metadata.is_dir() || metadata.file_type().is_symlink())
                {
                    target = Some((candidate, false));
                }
            }

            let entries = match fs::read_dir(&path) {
                Ok(entries) => entries,
                Err(error) => {
                    errors.push(format!("cannot read {}: {error}", path.display()));
                    break 'search;
                }
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(error) => {
                        errors.push(format!(
                            "cannot read an entry in {}: {error}",
                            path.display()
                        ));
                        continue;
                    }
                };
                let name = entry.file_name();
                let entry_path = entry.path();
                if (name == "target" && is_project)
                    || (!hidden
                        && name.to_string_lossy().starts_with('.')
                        && !is_stale(&entry_path))
                {
                    continue;
                }
                match entry.file_type() {
                    Ok(file_type) if file_type.is_dir() => children.push(entry_path),
                    Ok(_) => {}
                    Err(error) => {
                        errors.push(format!("cannot inspect {}: {error}", entry_path.display()))
                    }
                }
            }
        }

        'target: {
            let Some((target, stale)) = target else {
                break 'target;
            };
            if !shared
                .state
                .lock()
                .unwrap()
                .seen_targets
                .insert(target.clone())
            {
                break 'target;
            }

            println!(
                "found{}: {}",
                if stale { " stale" } else { "" },
                target.display()
            );
            let removal = if stale || dry {
                Removal {
                    display: target.clone(),
                    path: target,
                }
            } else {
                let parent = target.parent().unwrap();
                let stage = shared.next_stage.fetch_add(1, Ordering::Relaxed);
                let mut attempt = 0;
                let moved = loop {
                    let path = parent.join(format!(
                        ".cargo-quickclean-{}-{stage}-{attempt}",
                        shared.process_id
                    ));
                    match fs::symlink_metadata(&path) {
                        Ok(_) => attempt += 1,
                        Err(error) if error.kind() == io::ErrorKind::NotFound => break path,
                        Err(error) => {
                            errors.push(format!("cannot inspect {}: {error}", path.display()));
                            break 'target;
                        }
                    }
                };
                shared
                    .state
                    .lock()
                    .unwrap()
                    .seen_targets
                    .insert(moved.clone());
                if let Err(error) = fs::rename(&target, &moved) {
                    errors.push(format!("cannot move {}: {error}", target.display()));
                    break 'target;
                }
                Removal {
                    display: target,
                    path: moved,
                }
            };

            let mut state = shared.state.lock().unwrap();
            state.total += 1;
            state.removals.push_back(removal);
            drop(state);
            shared.wake.notify_one();
        }

        for error in &errors {
            eprintln!("cargo quickclean: {error}");
        }
        let mut state = shared.state.lock().unwrap();
        state.failed |= !errors.is_empty();
        for child in children {
            if state.seen_search.insert(child.clone()) {
                state.search.push_back(child);
            }
        }
        state.active_search -= 1;
        drop(state);
        shared.wake.notify_all();
    }
    (removed_bytes, complete)
}

fn absolute_path(path: PathBuf, current_directory: &Path) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        current_directory.join(path)
    }
}

fn is_stale(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with(".cargo-quickclean-"))
}

#[cfg(unix)]
fn allocated_size(metadata: &fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    metadata.blocks().saturating_mul(512)
}

#[cfg(not(unix))]
fn allocated_size(metadata: &fs::Metadata) -> u64 {
    metadata.len()
}

fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

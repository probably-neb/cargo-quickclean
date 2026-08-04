# cargo-quickclean

`cargo-quickclean` removes Cargo `target` directories without waiting for an incremental tree deletion.

Workers search and remove directories concurrently. When a worker finds a target, it atomically renames `target` to a temporary sibling path, then adds the detached tree to the removal queue.

The sibling path keeps the rename on the same file system. The rename is atomic for each target, but a run across multiple targets is not one transaction.

## Install

```sh
cargo install --path .
```

## Use

Pass one path, one shell glob, or multiple paths and globs:

```sh
cargo quickclean .
cargo quickclean code-dir/*
cargo quickclean work/* personal/*
```

The shell expands each glob before `cargo-quickclean` starts.

```text
Usage: cargo quickclean [OPTIONS] <PATHS>...

Arguments:
  <PATHS>...  Paths or globs to search for Cargo projects

Options:
  -H, --hidden           Search hidden directories
  -d, --dry              List and measure targets without removing them
  -i, --ignore <PATH>    Ignore a directory; this option can occur more than once
  -s, --no-size          Do not print removed sizes
  -h, --help             Print help
  -V, --version          Print version
```

The recursive search does not enter symbolic-link directory entries. A directory is a Cargo project when it contains `Cargo.toml`. The tool removes the conventional `target` directory beside that file. It does not read Cargo configuration for another target location.

The removed size is the sum of allocated blocks from entries that the tool removed. On non-Unix systems, it uses file lengths. Hard links can make this value differ from the change in free disk space.

If deletion stops after a rename, the temporary directory stays beside the project. Its name starts with `.cargo-quickclean-`. The next run finds and removes it.

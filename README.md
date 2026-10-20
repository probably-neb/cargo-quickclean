# cargo-quickclean

# Installation

```sh
cargo install --git https://github.com/probably-neb/cargo-quickclean
```

# Usage

```sh
cargo quickclean '~/code/*'
```

Quote globs so quickclean expands them instead of the shell. This avoids the
shell's argument-size limit. A leading `~` expands to your home directory:

```sh
cargo quickclean '*'
cargo quickclean 'repos/**/app-?'
cargo quickclean 'repos/{client,server}'
```

Each matched directory is searched recursively for Cargo projects to clean.
Patterns support `*` (any name), `?` (one byte), `**` (zero or more directories),
and `{a,b}` (alternatives, including paths). Unmatched globs find no targets.

Hidden directories are skipped unless explicitly named by the glob or enabled
with `--hidden`. `--ignore PATH` excludes a directory from both glob expansion
and project discovery. Directory symlinks are not followed.

Targets are normally renamed out of the way before deletion. If preparing or
renaming a target fails because the disk is full or a storage quota is exhausted,
quickclean deletes that target in place instead.
Avoid running builds during cleanup: in-place deletion can race with them.

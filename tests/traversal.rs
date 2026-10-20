use std::{fs, process::Command};

#[test]
fn clean_matching_projects() {
    let root = std::env::current_dir()
        .unwrap()
        .join("target")
        .join(format!("glob-traversal-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    let root = root.canonicalize().unwrap();
    let projects = ["repo-a", "repo-a/nested", "repo-b", ".hidden", "unselected"];
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("unselected"), root.join("repo-link")).unwrap();

    let absolute = root.join("repo-*");
    let cases: &[(&[&str], &[&str])] = &[
        (&["--dry", "*"], &projects),
        (&["*"], &[".hidden"]),
        (&["~"], &[".hidden"]),
        (&["~/repo-*"], &[".hidden", "unselected"]),
        (&[absolute.to_str().unwrap()], &[".hidden", "unselected"]),
        (&["repo-*"], &[".hidden", "unselected"]),
        (&["repo-{a,b}"], &[".hidden", "unselected"]),
        (&["{repo-{a,b},unselected}"], &[".hidden"]),
        (&["**/{nested,repo-b}"], &["repo-a", ".hidden", "unselected"]),
        (&["{repo-a/nested,repo-b}"], &["repo-a", ".hidden", "unselected"]),
        (&["**/**/nested"], &["repo-a", "repo-b", ".hidden", "unselected"]),
        (&["repo-?/nest*"], &["repo-a", "repo-b", ".hidden", "unselected"]),
        (&["repo-a/**"], &["repo-b", ".hidden", "unselected"]),
        (
            &["--ignore", "repo-a", "repo-*"],
            &["repo-a", "repo-a/nested", ".hidden", "unselected"],
        ),
        (
            &["--ignore", "~/repo-a", "~/repo-*"],
            &["repo-a", "repo-a/nested", ".hidden", "unselected"],
        ),
        (&["--hidden", "*"], &[]),
        (&["missing-*"], &projects),
    ];
    for (args, remaining) in cases {
        for project in projects {
            let directory = root.join(project);
            fs::create_dir_all(directory.join("target/debug/target")).unwrap();
            fs::write(directory.join("Cargo.toml"), "[workspace]\n").unwrap();
            fs::write(directory.join("target/debug/Cargo.toml"), "[workspace]\n").unwrap();
            fs::write(directory.join("target/debug/artifact"), "build output").unwrap();
        }
        let output = Command::new(env!("CARGO_BIN_EXE_cargo-quickclean"))
            .current_dir(&root)
            .env("HOME", &root)
            .args(["quickclean", "--no-size"])
            .args(*args)
            .output()
            .unwrap();
        assert_eq!(
            (output.status.code(), String::from_utf8(output.stderr).unwrap()),
            (Some(0), String::new()),
            "{args:?}",
        );
        let actual = projects
            .into_iter()
            .filter(|project| root.join(project).join("target").exists())
            .collect::<Vec<_>>();
        assert_eq!(actual, *remaining, "{args:?}");
    }
    for pattern in ["repo-{a,b", "repo-a}", "repo-{}"] {
        let output = Command::new(env!("CARGO_BIN_EXE_cargo-quickclean"))
            .current_dir(&root)
            .args(["quickclean", pattern])
            .output()
            .unwrap();
        assert_eq!(
            (output.status.code(), String::from_utf8(output.stderr).unwrap()),
            (
                Some(1),
                format!("cargo quickclean: invalid braces in {}\n", root.join(pattern).display()),
            ),
        );
    }
    fs::remove_dir_all(root).unwrap();
}

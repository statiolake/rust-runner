use failure::bail;
use failure::Fallible;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::HashSet;
use std::env;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::io::stdin;
use std::path::PathBuf;
use std::process::Command;
use tempfile::Builder;

lazy_static! {
    static ref RE_USE: Regex = Regex::new(r#"^\s*use\s+(?P<crate>[\w\d]+)"#).unwrap();
}

enum SourceFile {
    Path(PathBuf),
    Stdin,
}

impl SourceFile {
    fn read_content(&self) -> Fallible<String> {
        match self {
            SourceFile::Path(p) => {
                let mut buf = String::new();
                File::open(p)?.read_to_string(&mut buf)?;
                Ok(buf)
            }
            SourceFile::Stdin => {
                let mut buf = String::new();
                stdin().read_to_string(&mut buf)?;
                Ok(buf)
            }
        }
    }
}

struct Options {
    source_file: SourceFile,
}

impl Options {
    fn parse_args() -> Fallible<Options> {
        let source_file = match env::args().nth(1).as_ref().map(String::as_str) {
            Some("-") => SourceFile::Stdin,
            Some(p) => SourceFile::Path(PathBuf::from(p)),
            _ => SourceFile::Stdin,
        };

        Ok(Options { source_file })
    }
}

fn parse_imports(content: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for line in content.lines() {
        if let Some(captures) = RE_USE.captures(line) {
            set.insert(captures.name("crate").unwrap().as_str().into());
        }
    }

    set.remove("std");
    set
}

fn main() -> Fallible<()> {
    // 引数をパースする。
    let opts = Options::parse_args()?;

    // 内容を読み込み、インポートを抽出する。
    let content = opts.source_file.read_content()?;
    let imports = parse_imports(&content);

    // 一時ディレクトリにプロジェクトを作成し、そこへ移動。
    let tmpdir = Builder::new().prefix("rustjunk").tempdir()?;
    let old_current = env::current_dir()?;
    env::set_current_dir(tmpdir.path())?;

    // プロジェクトを初期化・実行する。
    let res = init_project(&content, &imports).and_then(|_| run_project());

    // 成否に関わらず一時ディレクトリを削除する。
    env::set_current_dir(old_current)?;

    res
}

fn init_project(content: &str, imports: &HashSet<String>) -> Fallible<()> {
    // cargo init
    let init_success = Command::new("cargo")
        .arg("init")
        .arg("--name")
        .arg("rustrunner")
        .status()?
        .success();
    if !init_success {
        bail!("failed to init cargo project.");
    }

    // ソースファイルを置き換える
    fs::remove_file("src/main.rs")?;
    let mut f = File::create("src/main.rs")?;
    f.write_all(content.as_bytes())?;

    // 必要なクレートを `cargo add` する
    for import in imports {
        eprintln!("adding `{}` to the project", import);
        let success = Command::new("cargo")
            .arg("add")
            .arg(import)
            .status()?
            .success();
        if !success {
            eprintln!("  ... adding crate `{}` failed, ignoring.", import);
        }
    }

    Ok(())
}

fn run_project() -> Fallible<()> {
    let success = Command::new("cargo").arg("run").status()?.success();
    if !success {
        bail!("failed to run the program.");
    }

    Ok(())
}

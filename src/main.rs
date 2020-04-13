use failure::bail;
use failure::Fallible;
use itertools::Itertools;
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::io::stdin;
use std::path::PathBuf;
use std::process::Command;
use tempfile::Builder;

lazy_static! {
    static ref RE_OPTION_COMMENT: Regex =
        Regex::new(r#"^\s*//\s*rust-runner:\s*(?P<option>.*)$"#).unwrap();
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

struct Args {
    source_file: SourceFile,
}

impl Args {
    fn parse_args(args: &[&str]) -> Fallible<Args> {
        let source_file = match args.get(1).copied() {
            Some("-") => SourceFile::Stdin,
            Some(p) => SourceFile::Path(PathBuf::from(p)),
            _ => SourceFile::Stdin,
        };

        Ok(Args { source_file })
    }
}

struct Context {
    toolchain: String,
    imports: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum OptionType {
    Toolchain,
}

impl OptionType {
    fn parse(name: &str) -> Option<OptionType> {
        match name {
            "toolchain" => Some(OptionType::Toolchain),
            _ => None,
        }
    }
}

impl Context {
    pub fn parse(content: &str) -> Fallible<Context> {
        let options = Context::gather_options(content)?;
        let toolchain = Context::parse_toolchain(&options).to_string();
        let imports = Context::parse_imports(content);

        Ok(Context { toolchain, imports })
    }

    /// プログラム先頭のコメント行にあるオプション指定をパースし、 OptionType の配列にする
    fn gather_options(content: &str) -> Fallible<HashMap<OptionType, String>> {
        let mut options = HashMap::new();
        for line in content.lines() {
            if line.trim() == "" {
                // 空行は飛ばす
                continue;
            }

            if !line.trim_start().starts_with("//") {
                // 先頭のコメント行が終わったのでパースを終わる
                break;
            }

            let captures = match RE_OPTION_COMMENT.captures(line) {
                Some(options) => options,
                None => continue,
            };

            let option_strs = captures.name("option").unwrap().as_str().split(';');
            for option_str in option_strs {
                let mut name_value = option_str.splitn(2, '=').fuse();
                let name = name_value.next();
                let value = name_value.next();
                let (name, value) = match (name, value) {
                    (Some(name), Some(value)) => (name, value),
                    _ => bail!("invalid option string: {}", option_str),
                };

                match OptionType::parse(name) {
                    Some(option_type) => {
                        options.insert(option_type, value.into());
                    }
                    None => bail!("unknown option: {}", name),
                }
            }
        }

        Ok(options)
    }

    fn parse_toolchain(options: &HashMap<OptionType, String>) -> &str {
        options
            .get(&OptionType::Toolchain)
            .map(String::as_str)
            .unwrap_or("stable")
    }

    fn parse_imports(content: &str) -> HashSet<String> {
        let mut set = HashSet::new();
        for line in content.lines() {
            if let Some(captures) = RE_USE.captures(line) {
                set.insert(captures.name("crate").unwrap().as_str().into());
            }
        }

        for special in &["std", "crate", "self", "super"] {
            set.remove(*special);
        }

        set
    }
}

fn main() -> Fallible<()> {
    // 引数をパースする。
    let args = env::args().collect_vec();
    let args = args.iter().map(String::as_str).collect_vec();
    let args = Args::parse_args(&args)?;

    // 内容を読み込み、インポートを抽出する。
    let content = args.source_file.read_content()?;
    let context = Context::parse(&content)?;

    // 一時ディレクトリにプロジェクトを作成し、そこへ移動。
    let tmpdir = Builder::new().prefix("rustjunk").tempdir()?;
    let old_current = env::current_dir()?;
    env::set_current_dir(tmpdir.path())?;

    // プロジェクトを初期化・実行する。
    let res = init_project(&content, &context).and_then(|_| run_project());

    // 成否に関わらず一時ディレクトリを削除する。
    env::set_current_dir(old_current)?;

    res
}

fn init_project(content: &str, context: &Context) -> Fallible<()> {
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

    // sccache が使える場合は sccache を有効にする
    if let Ok(sccache) = which::which("sccache") {
        fs::create_dir_all(".cargo")?;
        let mut s = Vec::new();
        writeln!(s, r#"[build]"#).unwrap();
        writeln!(
            s,
            r#"rustc-wrapper = "{}""#,
            sccache.display().to_string().escape_default()
        )
        .unwrap();
        fs::write(".cargo/config", s)?;
    }

    // ソースファイルを置き換える
    fs::remove_file("src/main.rs")?;
    let mut f = File::create("src/main.rs")?;
    f.write_all(content.as_bytes())?;

    // 必要なクレートを `cargo add` する
    for import in &context.imports {
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

    // rust-toolchain を書き込む
    fs::write("rust-toolchain", &context.toolchain)?;

    Ok(())
}

fn run_project() -> Fallible<()> {
    let success = Command::new("cargo").arg("run").status()?.success();
    if !success {
        bail!("failed to run the program.");
    }

    Ok(())
}

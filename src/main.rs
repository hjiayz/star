//! simple tar archive compression/decompression tool
//!
//! # Install
//!
//! ```
//!     cargo install star
//! ```
//!
//! # Example
//!
//! ```
//! star c foo.xz Cargo.toml to foo/
//!
//! star c foo.xz from ./**/*.dll to lib/ from ./**/*.exe to bin/
//!
//! star c foo.xz Cargo.toml to foo/Bar.toml
//!
//! star x foo.xz
//!
//! star x foo.xz bar/
//! ```
//!
//! # more
//!
//! star --help
//!

use clap::{crate_authors, crate_version, App, Arg, SubCommand};
use std::fs::File;
use std::io::Read;
use std::io::Write;
use std::path::{Path, PathBuf};

fn main() {
    let ar_arg = Arg::with_name("archive")
        .value_name("FILE_PATH")
        .required(true)
        .index(1)
        .help("archive file path");
    let app = App::new("star")
        .version(crate_version!())
        .author(crate_authors!())
        .about("archive tool")
        .arg(
            Arg::with_name("format")
                .value_name("FORMAT")
                .short("f")
                .help("archive file format")
                .possible_values(&["xz", "tar", "zst", "zstd", "gzip", "gz", "z", "tgz"]),
        )
        .arg(
            Arg::with_name("compression_only")
                .short("c")
                .help("compression/decompression only.no tar archive."),
        )
        .subcommand(
            SubCommand::with_name("c")
                .about("new archive")
                .arg(ar_arg.clone())
                .arg(
                    Arg::with_name("append")
                        .value_name("[from ]APPEND_PATH[ to NEW_PATH]")
                        .takes_value(true)
                        .multiple(true)
                        .required(true)
                        .help("append file to new package.allow glob and rename."),
                ),
        )
        .subcommand(
            SubCommand::with_name("x")
                .about("extract archive")
                .arg(ar_arg)
                .arg(
                    Arg::with_name("extract")
                        .value_name("EXTRACT_DIR")
                        .default_value("./")
                        .takes_value(true)
                        .help("extract to the path."),
                ),
        );
    let mut help = Vec::new();
    app.write_help(&mut help).unwrap();
    let matches = app.get_matches();
    let format_type = matches.value_of("format");
    let compression_only = matches.is_present("compression_only");
    if let Some(ref smatches) = matches.subcommand_matches("c") {
        let archive: &Path = smatches.value_of("archive").unwrap().as_ref();
        let append = smatches.values_of("append").unwrap();
        let format_type = check_format_type(format_type, archive);
        if format_type.is_none() {
            println!("unknown format");
            std::io::stdout().write_all(&help).unwrap();
            return;
        }
        create(format_type.unwrap(), append, archive, compression_only);
        return;
    }
    if let Some(ref smatches) = matches.subcommand_matches("x") {
        let archive: &Path = smatches.value_of("archive").unwrap().as_ref();
        let dst: &str = smatches.value_of("extract").unwrap();
        let format_type = check_format_type(format_type, archive);
        if format_type.is_none() {
            println!("unknown format");
            std::io::stdout().write_all(&help).unwrap();
            return;
        }
        extract(format_type.unwrap(), archive, dst, compression_only);
        return;
    }
}

fn append<W: Write>(
    ar: &mut tar::Builder<W>,
    src: Box<dyn Iterator<Item = Result<PathBuf, glob::GlobError>>>,
    target: Option<&Path>,
) {
    for path in src {
        let path = path.unwrap();
        let mut buf;
        let mut target = *target.as_ref().unwrap_or(&path.as_ref());
        if target_is_dir(target) {
            buf = target.to_path_buf();
            buf.push(path.file_name().unwrap());
            target = buf.as_path();
        }
        if path.is_dir() {
            ar.append_dir_all(&target, &path).unwrap();
            println!(
                "dir {} to {}",
                path.to_string_lossy(),
                target.to_string_lossy()
            );
            continue;
        }

        ar.append_path_with_name(&path, &target).unwrap();
        println!(
            "file {} to {}",
            path.to_string_lossy(),
            target.to_string_lossy()
        );
    }
}

fn check_format_type(format_type: Option<&str>, path: &Path) -> Option<&'static str> {
    let t = format_type
        .or_else(|| Some(path.extension()?.to_str()?))?
        .to_lowercase();
    Some(match t.as_str() {
        "xz" => "xz",
        "gzip" | "gz" | "tgz" | "z" => "gzip",
        "tar" => "tar",
        "zst" | "zstd" => "zstd",
        _ => None?,
    })
}

fn get_encoder(file_type: &str, file: File) -> Box<dyn Write> {
    match file_type {
        "xz" => Box::new(xz2::write::XzEncoder::new(file, 9)),
        "gzip" => Box::new(flate2::write::GzEncoder::new(
            file,
            flate2::Compression::best(),
        )),
        "tar" => Box::new(file),
        "zstd" => Box::new(
            zstd::stream::write::Encoder::new(file, 21)
                .expect("faild to create zstd encoder")
                .auto_finish(),
        ),
        _ => unreachable!("unknown file type"),
    }
}

fn create(
    format_type: &str,
    mut append_files: clap::Values,
    filepath: &Path,
    compression_only: bool,
) {
    if filepath.exists() {
        panic!("file path {} exists", filepath.display());
    }

    let file = std::fs::File::create(filepath).unwrap();
    let mut encoder = get_encoder(format_type, file);
    if compression_only {
        let param = append_files.next().expect("source file no exists");
        if append_files.next().is_some() {
            panic!("more than one file. can not compression only");
        }
        let mut src_file = File::open(
            glob::glob(param)
                .unwrap()
                .next()
                .expect("source file no exists")
                .unwrap(),
        )
        .unwrap();
        let _ = std::io::copy(&mut src_file, &mut encoder).unwrap();
        drop(encoder);
        println!("{} created.", filepath.to_str().unwrap());
        return;
    }
    let mut ar = tar::Builder::new(encoder);
    let mut paths: Option<Box<dyn Iterator<Item = Result<PathBuf, glob::GlobError>>>> = None;
    let mut from = false;
    let mut to = false;
    for param in append_files {
        let lowcase = param.to_lowercase();
        if lowcase == "from" {
            from = true;
            continue;
        }
        if to {
            append(&mut ar, paths.take().unwrap(), Some(param.as_ref()));
            to = false;
            continue;
        }
        if paths.is_some() {
            if lowcase == "to" {
                to = true;
                from = false;
                continue;
            }
            if from {
                paths = paths.map(|src| {
                    Box::new(src.chain(glob::glob(param).unwrap()))
                        as Box<dyn Iterator<Item = Result<PathBuf, glob::GlobError>>>
                });
                continue;
            }
            append(&mut ar, paths.take().unwrap(), None);
            continue;
        }
        paths = Some(Box::new(glob::glob(param).unwrap()));
    }
    if paths.is_some() {
        append(&mut ar, paths.take().unwrap(), None);
    }
    ar.finish().unwrap();
    println!("{} created.", filepath.to_str().unwrap());
}

fn get_decoder(file_type: &str, file: File) -> Box<dyn Read> {
    match file_type {
        "xz" => Box::new(xz2::read::XzDecoder::new(file)),
        "gzip" => Box::new(flate2::read::GzDecoder::new(file)),
        "tar" => Box::new(file),
        "zstd" => {
            Box::new(zstd::stream::read::Decoder::new(file).expect("faild to create zstd encoder"))
        }
        _ => unreachable!("unknown file type"),
    }
}

fn extract(format_type: &str, filepath: &Path, dst: &str, compression_only: bool) {
    let dst: &Path = dst.as_ref();
    if dst.exists() & (!dst.is_dir()) {
        panic!("dst path {} exists", dst.display());
    }
    let file = std::fs::File::open(filepath).unwrap();
    let mut decoder = get_decoder(format_type, file);
    if compression_only {
        let mut dstfile = File::create(dst).unwrap();
        let _ = std::io::copy(&mut decoder, &mut dstfile).unwrap();
        println!("ok.");
        return;
    }
    let mut ar = tar::Archive::new(decoder);
    ar.unpack(dst).unwrap();
    println!("ok.")
}

fn target_is_dir(path: &Path) -> bool {
    let path = path.to_string_lossy().to_string();
    if path.len() == 0 {
        return true;
    }
    if path.ends_with("/") {
        return true;
    }
    if cfg!(windows) && path.ends_with("\\") {
        return true;
    }
    false
}

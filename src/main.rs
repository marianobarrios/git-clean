mod util;

use anyhow::{Context, Result, bail};
use bytesize::ByteSize;
use clap::Parser;
use console::{Term, style};
use dialoguer::Confirm;
use git2::{Repository, Status, StatusEntry, StatusOptions};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use std::path::{Path, PathBuf};
use std::{env, fs};
use tracing::{debug, info, info_span, trace, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "git-clean",
    about = "Delete untracked and repo-ignored files from all git repositories under a directory",
    long_about = "Recursively finds all git repositories under a root directory and removes \
files that Git considers untracked or repo-locally ignored.\n\n\
Two layers of gitignore rules are in play:\n\
  - The GLOBAL gitignore (core.excludesFile, typically ~/.gitignore_global or ~/.config/git/ignore) \
lists patterns to be ignored in every repository, usually IDE metadata, OS noise, etc. Files \
matching the global gitignore are PROTECTED and will NOT be removed.\n\
  - Each repository's own .gitignore usually lists build artifacts, generated files, and \
other repo-specific noise. Files matching ONLY the repo gitignore are candidates for removal.\n\n\
Symbolic links to directories are not traversed, but a symlink that points directly to a Git \
repository root is recognised and included.",
    after_help = "EXAMPLES:
  git-clean                  dry-run from current directory
  git-clean ~/projects       dry-run across all repos under ~/projects
  git-clean --execute        delete untracked + repo-ignored files (with confirmation)
  git-clean --execute -y     delete without prompting"
)]
struct Args {
    /// Base directory to search (default: current directory)
    base_dir: Option<PathBuf>,

    /// Actually delete files. Without this flag the tool is a no-op dry-run.
    #[arg(long)]
    execute: bool,

    /// Skip confirmation prompt when --execute is set.
    #[arg(short = 'y', long)]
    yes: bool,

    /// Increase log verbosity (-v = info, -vv = debug, -vvv = trace).
    #[arg(short, long, action = clap::ArgAction::Count)]
    verbose: u8,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let level = match args.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    ctrlc::set_handler(|| {
        let _ = Term::stdout().show_cursor();
        std::process::exit(130);
    })
    .context("failed to install Ctrl-C handler")?;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(format!("{level},globset=info,ignore=info")));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    // The ignore crate reparses the global gitignore for every repository it checks.
    // We log it once here so the user can verify which file is being used as a shield.
    if let Some(path) = ignore::gitignore::gitconfig_excludes_path() {
        info!("global gitignore (protected): {}", path.display());
        let contents = fs::read_to_string(&path)?;
        for line in contents.lines() {
            debug!("  {line}");
        }
    } else {
        info!("no global gitignore found — all repo-ignored files are candidates for removal");
    }

    let root_path =
        args.base_dir.unwrap_or_else(|| env::current_dir().expect("cannot get current directory"));
    let root_path =
        root_path.canonicalize().with_context(|| format!("cannot open directory {}", root_path.display()))?;

    if !root_path.is_dir() {
        bail!("{} is not a directory", root_path.display());
    }

    info!("searching for Git repositories under: {}", root_path.display());
    let repos = find_git_repos(&root_path)?;
    if repos.is_empty() {
        println!("{}", style("No Git repositories found.").yellow());
        return Ok(());
    }
    println!(
        "Found {} Git {}.",
        style(repos.len()).bold(),
        if repos.len() == 1 { "repository" } else { "repositories" }
    );

    let groups = collect_all_for_deletion(&repos)?;
    let all_paths: Vec<PathBuf> =
        groups.iter().flat_map(|(repo, paths)| paths.iter().map(|p| repo.join(p))).collect();

    if all_paths.is_empty() {
        println!("{}", style("No paths for deletion.").dim());
        return Ok(());
    }

    println!("{}", style("Paths for deletion:").bold());
    if let Some(repo) = repos.first()
        && *repo == root_path
    {
        // if only one repository was found in the root, show the paths right away
        let (_, paths) = groups.first().unwrap();
        for path in paths {
            println!("  {}", style(path.display()).red());
        }
    } else {
        for (repo_path, paths) in &groups {
            println!("  {}:", style(repo_path.strip_prefix(&root_path).unwrap().display()).bold());
            for path in paths {
                println!("    {}", style(path.display()).red());
            }
        }
    }

    let mut total_size = ByteSize::default();
    for path in &all_paths {
        total_size += ByteSize(dir_size(path)?);
    }

    if args.execute {
        if !args.yes {
            let confirm = Confirm::new().with_prompt("Proceed with deletion?").default(false);
            if !confirm.interact().unwrap_or(false) {
                println!("{}", style("Aborted.").yellow());
                return Ok(());
            }
        }
        for path in &all_paths {
            delete_path(path)?;
        }
        println!("{} item(s) removed — {}", style(all_paths.len()).green().bold(), style(total_size).blue());
    } else {
        println!(
            "{} item(s) would be removed — {}. Pass {} to actually delete them.",
            style(all_paths.len()).yellow().bold(),
            style(total_size).blue(),
            style("--execute").bold(),
        );
    }

    Ok(())
}

/// Collects paths to remove across all repositories, grouped by repository.
///
/// For each repository, the global gitignore is loaded and used as a shield: any path
/// that matches a global pattern is excluded from the candidate list even if Git itself
/// would mark it as ignored.
fn collect_all_for_deletion(repos: &[PathBuf]) -> Result<Vec<(PathBuf, Vec<PathBuf>)>> {
    let mut groups = Vec::new();
    for repo_path in repos {
        let repo = Repository::open(repo_path)?;

        let repo_name = repo.workdir().context("called on bare repo")?.to_string_lossy().into_owned();
        let _span = info_span!("repo", repo = %repo_name).entered();

        // build_global() reads core.excludesFile (or the standard fallback locations) and
        // compiles it into a matcher that is then passed into paths_to_clean as a shield.
        let (global_gitignore, err) = GitignoreBuilder::new(repo_path).build_global();
        if let Some(e) = err {
            bail!(e);
        }

        debug!("inspecting repo {}", repo_path.display());
        let paths = paths_to_clean(&repo, &global_gitignore)?;
        if !paths.is_empty() {
            groups.push((repo_path.clone(), paths));
        }
    }
    Ok(groups)
}

/// Walk the filesystem looking for Git repositories, using `Repository::open` to validate each
/// directory. Symlinks to directories are not traversed, but a symlink that points directly to a
/// Git repository root is recognized and included.
fn find_git_repos(root: &Path) -> Result<Vec<PathBuf>> {
    let mut repos = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        trace!("scanning {}", dir.display());
        if is_non_bare_git_repo(&dir) {
            repos.push(dir);
        } else {
            for dir_entry in
                fs::read_dir(&dir).with_context(|| format!("cannot read directory {}", dir.display()))?
            {
                let dir_entry = dir_entry?;
                let file_type = dir_entry
                    .file_type()
                    .with_context(|| format!("cannot get file type of {:?}", dir_entry))?;
                if file_type.is_dir() {
                    stack.push(dir_entry.path());
                } else if file_type.is_symlink() {
                    // Don't recurse through the symlink, but do check if it points to a repo.
                    let path = dir_entry.path();
                    if is_non_bare_git_repo(&path) {
                        repos.push(path);
                    } else {
                        trace!("skipping non-repo at {}", path.display());
                    }
                }
            }
        }
    }
    repos.sort();
    Ok(repos)
}

fn is_non_bare_git_repo(path: &Path) -> bool {
    match Repository::open(path) {
        Ok(repo) if !repo.is_bare() => true,
        Ok(_) => {
            debug!("skipping bare repo: {}", path.display());
            false
        }
        Err(_) => false,
    }
}

/// Calculate the total recursive size of a directory, does not follow symlinks
fn dir_size(path: &Path) -> Result<u64> {
    let meta = path.symlink_metadata()?;
    if meta.is_file() || meta.file_type().is_symlink() {
        return Ok(meta.len());
    }
    let mut total = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            total += dir_size(&entry.path())?;
        } else {
            total += entry.path().symlink_metadata()?.len();
        }
    }
    Ok(total)
}

/// Returns paths inside `repo` that are candidates for removal.
///
/// A path is a candidate when Git marks it as untracked (`WT_NEW`) or repo-locally
/// ignored (`IGNORED`), AND it does not match any pattern in `global_gitignore`..
fn paths_to_clean(repo: &Repository, global_gitignore: &Gitignore) -> Result<Vec<PathBuf>> {
    let workdir = repo.workdir().context("called on bare repo")?;

    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .include_ignored(true)
        .exclude_submodules(true)
        .recurse_untracked_dirs(false) // surface untracked dirs as one entry
        .recurse_ignored_dirs(false); // surface ignored dirs as one entry

    let statuses = repo.statuses(Some(&mut opts))?;

    let mut paths = Vec::new();
    for entry in &statuses {
        if !entry.status().intersects(Status::WT_NEW | Status::IGNORED) {
            continue;
        }
        let (path, is_dir) = to_path(entry);
        let abs_path = workdir.join(&path);
        if !global_gitignore.matched_path_or_any_parents(&abs_path, is_dir).is_ignore() {
            debug!("path to clean: {}", path.display());
            paths.push(path);
        }
    }
    Ok(paths)
}

fn to_path(entry: StatusEntry) -> (PathBuf, bool) {
    let path = entry.path().expect("non-utf-8 path");
    trace!("processing local file: {}", path);
    let is_dir = path.ends_with('/');
    let path = path.trim_end_matches('/');
    (PathBuf::from(path), is_dir)
}

/// Deletes a path, recursively if it is a directory. Does not follow symlinks.
fn delete_path(path: &Path) -> Result<()> {
    info!("deleting {}", path.display());
    let is_dir = path
        .symlink_metadata()
        .with_context(|| format!("cannot stat {}", path.display()))?
        .file_type()
        .is_dir();
    if let Err(e) = if is_dir { fs::remove_dir_all(path) } else { fs::remove_file(path) } {
        warn!("failed to delete {}: {}", path.display(), e);
    }
    Ok(())
}

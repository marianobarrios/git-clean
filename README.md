# git-clean

[![CI](https://github.com/marianobarrios/git-clean/actions/workflows/ci.yml/badge.svg)](https://github.com/marianobarrios/git-clean/actions/workflows/ci.yml)

**git-clean** recursively scans a directory tree for Git repositories and removes files that Git considers untracked or repo-locally ignored — build artifacts, generated files, editor noise, and the like.

## How it works

Two layers of gitignore rules are in play:

- The **global gitignore** (`core.excludesFile`, typically `~/.gitignore_global` or `~/.config/git/ignore`) lists patterns you want ignored across every repository on your machine — IDE metadata, OS noise, etc. Files matching the global gitignore are **protected** and will never be removed, even if Git itself would consider them ignored.

- Each **repository's own `.gitignore`** lists build artifacts, generated files, and other repo-specific noise. Files matching only the repo gitignore are **candidates for removal**.

This means git-clean is safe to run broadly: your dotfiles, IDE configs, and other machine-wide ignores act as a shield.

## Installation

**Prerequisites:** [Rust](https://rustup.rs).

```sh
cargo install --path .
```

Or build and run directly:

```sh
cargo run --release -- [OPTIONS] [ROOT]
```

## Usage

```
git-clean [OPTIONS] [ROOT]
```

| Argument / Option | Description                                                          |
|-------------------|----------------------------------------------------------------------|
| `[BASE_DIR]`      | Base directory to search (default: current directory)                |
| `--execute`       | Actually delete files. Without this flag the tool is a no-op dry-run |
| `-y, --yes`       | Skip the confirmation prompt when `--execute` is set                 |
| `-v, --verbose`   | Increase log verbosity (`-v` = info, `-vv` = debug, `-vvv` = trace)  |

### Examples

```sh
# Dry-run from the current directory — shows what would be removed
git-clean

# Dry-run across all repositories under ~/projects
git-clean ~/projects

# Delete files (asks for confirmation)
git-clean --execute

# Delete files without prompting
git-clean --execute -y
```

## Works well with

[**git-workspace**](https://github.com/orf/git-workspace) clones and keeps in sync an entire organisation's repositories from GitHub, GitLab, or Gitea into a single local directory tree. git-clean is a natural companion: use git-workspace to maintain an up-to-date local mirror, then run git-clean against the same directory to keep it tidy.

```sh
# Sync all repositories
git-workspace update

# Remove build artifacts and ignored files across all repos
git-clean ~/workspace --execute
```

## License

MIT

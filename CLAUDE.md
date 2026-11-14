# Working on Afterglow

How this repo is worked on. See `README.md` for what the app does and how to build it.

## Commits

- Other changes may be in progress in the working tree at the same time. Commit only your own
  files, by path: `git commit -- <files>`. Never run a bare `git commit` after `git add`.
  Check `git status` and `git log` right before committing.
- Work happens on `develop`. `develop` is merged into `main` only when a release happens (see
  `docs/releasing.md`). Don't push to `main` otherwise.
- Commit messages are a plain sentence describing the change ("Let Enter press the button over
  the video, styled like the Skip button"). Version bumps are just "Version X.Y.Z".
- No `Co-Authored-By` trailers or other AI attribution in commits. The README says how the
  project is made instead.

## Licensing

Afterglow is free software under GPL-3.0-or-later (`LICENSE`), which lets release builds
include shinchiro's GPL libmpv. Code and assets added to the repo must be compatible with
GPL-3.0-or-later.

Afterglow updates itself straight from GitHub releases (`src/update.rs`), and only installs a
release signed with the maintainer's key, so every release must be signed (see
`docs/releasing.md`). A published release reaches every copy, so never publish an untested one.

## Ideas

Parked ideas are in `docs/ideas.md`. Don't implement them until the owner says so, but bring
them up when asked what's left to do.

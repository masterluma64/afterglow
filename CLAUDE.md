# Working on afterglow

How this repo is worked on. See `README.md` for what the app does and how to build it.

## Commits

- Others (the owner, other sessions) edit and stage changes in this repo at the same time.
  Commit only your own files, by path: `git commit -- <files>`. Never run a bare `git commit`
  after `git add`. Check `git status` and `git log` right before committing, because the repo
  may have moved since you last looked.
- Work happens on `develop`. `develop` is merged into `main` only when a release happens (see
  `docs/releasing.md`). Don't push to `main` otherwise.
- Commit messages are a plain sentence describing the change ("Let Enter press the button over
  the video, styled like the Skip button"). Version bumps are just "Version X.Y.Z".
- No `Co-Authored-By` trailers or other AI attribution in commits. The README says how the
  project is made instead.

## Naming

- Use lowercase `afterglow` in file names, folder names, archive names and other technical
  identifiers (`APP_ID`, the `Pictures\afterglow` screenshot folder, release assets).
- The capitalized "Afterglow" stays in UI text (`APP_NAME`), README prose and code comments.

## Licensing and distribution

afterglow is free software under GPL-3.0-or-later (`LICENSE`), chosen in November 2026 to
prepare for a public release. That is what lets release builds include shinchiro's GPL
libmpv. Code and assets added to the repo must be compatible with GPL-3.0-or-later.

Options weighed before, and set aside: an LGPL libmpv build to stay closed source (needs a
portable MSYS2 to build), and downloading libmpv on first start (a licensing grey area, with
no reliable host).

Self-updating is wanted later, straight from GitHub releases (no server in between). Release
archives are already signed for it (see `docs/releasing.md`).

## Ideas

Parked ideas are in `docs/ideas.md`. Don't implement them until the owner says so, but bring
them up when asked what's left to do.

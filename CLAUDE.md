# Working on afterglow

How this repo is worked on. See `README.md` for what the app does and how to build it.

## Commits

- Others (the owner, other sessions) edit and stage changes in this repo at the same time.
  Commit only your own files, by path: `git commit -- <files>`. Never run a bare `git commit`
  after `git add`. Check `git status` and `git log` right before committing, because the repo
  may have moved since you last looked.
- Work happens on `develop`. `main` is brought up to date by fast-forwarding it to `develop`
  (`git push origin develop:main`), at every release and when the owner asks.
- Commit messages are a plain sentence describing the change ("Let Enter press the button over
  the video, styled like the Skip button"). Version bumps are just "Version X.Y.Z".

## Naming

- Use lowercase `afterglow` in file names, folder names, archive names and other technical
  identifiers (`APP_ID`, the `Pictures\afterglow` screenshot folder, release assets).
- The capitalized "Afterglow" stays in UI text (`APP_NAME`), README prose and code comments.

## Licensing and distribution

afterglow is closed source and only for the owner's own use: no testers, no public release.
So the GPL libmpv build from shinchiro stays, since the GPL only applies when copies go to
others. The GitHub repo is private, which is why release builds with libmpv packed in may be
attached to its releases.

Don't suggest an LGPL libmpv, a licence change or publishing unless the owner brings up
sharing again. If they do, these were weighed in October 2026:

- An LGPL libmpv build. Building it needs a portable MSYS2; MSVC alone isn't enough.
- GPL-3.0-or-later in public. The history was checked and is clean.
- Downloading libmpv on first start. Rejected: a licensing grey area, with no reliable host.

## Ideas

Parked ideas are in `docs/ideas.md`. Don't implement them until the owner says so, but bring
them up when asked what's left to do.

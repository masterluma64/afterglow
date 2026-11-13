# Updates

afterglow updates itself from the GitHub releases of this private repo, through a
Cloudflare Worker that hands out the releases and nothing else. It is for the owner's own
machines only (see "Licensing and distribution" in `CLAUDE.md`).

## How it works

1. On start (or with *Check now* in Settings), afterglow asks the Worker for the newest
   release, sending its key.
2. If it is newer, afterglow downloads the `.7z` and its `.sig` in the background. The
   Worker only redirects to GitHub's download link, which expires within minutes, so
   neither the GitHub token nor the repo is exposed.
3. afterglow checks the signature against the public key built into it (`PUBLIC_KEY` in
   `src/update.rs`) and unpacks the exe into `data\update`. An archive signed with another
   key is never installed.
4. **Restart to update** shows at the top right. Clicking it renames the running exe to
   `afterglow.old.exe`, puts the new one in its place, starts it, and quits. The new copy
   waits for the old one to close, removes what the update left behind, and shows the
   release notes under "Updated to Afterglow X.Y.Z".

## Setting up the Worker

Needs a Cloudflare account and Node.js. From `worker/`:

1. Make a fine-grained GitHub token for this repo only, with **Contents: Read-only**.
2. Deploy and set the secrets:

   ```sh
   npx wrangler login
   npx wrangler deploy
   npx wrangler secret put GITHUB_TOKEN    # the token from step 1
   npx wrangler secret put CLIENT_KEYS     # one long random key per machine, comma-separated
   ```

3. In afterglow's Settings, under Updates, enter the Worker's address (the
   `https://afterglow-updates.<account>.workers.dev` that `deploy` prints) and that
   machine's key.

To lock a machine out, remove its key from `CLIENT_KEYS`.

## Signing key

Releases are signed with an ed25519 key kept in
`%APPDATA%\afterglow-release\signing.key`, outside the repo. Keep a copy somewhere safe:
without it, new releases can't be signed with the key existing copies trust, and those
copies would have to be updated by hand once. `cargo run --example sign_release -- new-key`
makes a new key (it won't replace an existing one); its public half then goes in
`PUBLIC_KEY`. Signing is a step in `docs/releasing.md`.

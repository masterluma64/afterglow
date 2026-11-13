// Hands out afterglow's releases from the private GitHub repo, so a copy of
// afterglow can update itself without a way into the repo. It holds a
// read-only GitHub token (the GITHUB_TOKEN secret) that never leaves it, and
// answers only copies that send one of the keys in the CLIENT_KEYS secret
// (comma-separated). See docs/updates.md.
//
//   GET /latest                    the newest release: version, notes, assets
//   GET /download/vX.Y.Z/<asset>   a redirect to a short-lived download link
//
// Anything else is a 404, so the GitHub API can't be reached through it.

const REPO = "masterluma64/afterglow";
/// How long the newest release is kept before GitHub is asked again.
const LATEST_TTL = 300;
/// Release assets that may be downloaded: the archive and its signature.
const ASSET = /^afterglow-\d+\.\d+\.\d+-windows-x64\.7z(\.sig)?$/;

export default {
  async fetch(request, env, ctx) {
    if (request.method !== "GET") {
      return text("Method not allowed", 405);
    }
    if (!authorized(request, env)) {
      return text("Unauthorized", 401);
    }
    const path = new URL(request.url).pathname;
    if (path === "/latest") {
      return latest(env, ctx);
    }
    const download = path.match(/^\/download\/(v\d+\.\d+\.\d+)\/([^/]+)$/);
    if (download && ASSET.test(download[2])) {
      return asset(env, download[1], download[2]);
    }
    return text("Not found", 404);
  },
};

function authorized(request, env) {
  const header = request.headers.get("Authorization") ?? "";
  const key = header.startsWith("Bearer ") ? header.slice(7).trim() : "";
  const keys = (env.CLIENT_KEYS ?? "").split(",").map((k) => k.trim()).filter(Boolean);
  return key !== "" && keys.some((k) => equal(k, key));
}

/// Compares in a time that doesn't depend on where they differ.
function equal(a, b) {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}

function github(env, path, accept = "application/vnd.github+json") {
  return fetch(`https://api.github.com/repos/${REPO}${path}`, {
    headers: {
      Authorization: `Bearer ${env.GITHUB_TOKEN}`,
      Accept: accept,
      "User-Agent": "afterglow-updates",
      "X-GitHub-Api-Version": "2022-11-28",
    },
    redirect: "manual",
  });
}

/// The newest release, kept for a few minutes so GitHub's rate limit is
/// never near.
async function latest(env, ctx) {
  const cache = caches.default;
  const key = new Request("https://afterglow-updates.internal/latest");
  const cached = await cache.match(key);
  if (cached) return cached;
  const response = await github(env, "/releases/latest");
  if (!response.ok) {
    return text(`GitHub answered ${response.status}`, 502);
  }
  const release = await response.json();
  const body = {
    version: release.tag_name.replace(/^v/, ""),
    tag: release.tag_name,
    name: release.name,
    notes: release.body ?? "",
    published: release.published_at,
    assets: release.assets.filter((a) => ASSET.test(a.name)).map((a) => ({ name: a.name, size: a.size })),
  };
  const result = new Response(JSON.stringify(body), {
    headers: { "Content-Type": "application/json", "Cache-Control": `max-age=${LATEST_TTL}` },
  });
  ctx.waitUntil(cache.put(key, result.clone()));
  return result;
}

/// A redirect to GitHub's download link for a release asset. The link is
/// signed and runs out within minutes, and the file doesn't pass through
/// the Worker.
async function asset(env, tag, name) {
  const release = await github(env, `/releases/tags/${tag}`);
  if (!release.ok) {
    return text("No such release", 404);
  }
  const found = (await release.json()).assets.find((a) => a.name === name);
  if (!found) {
    return text("No such file", 404);
  }
  const download = await github(env, `/releases/assets/${found.id}`, "application/octet-stream");
  const location = download.headers.get("Location");
  if (download.status < 300 || download.status >= 400 || !location) {
    return text(`GitHub answered ${download.status}`, 502);
  }
  return Response.redirect(location, 302);
}

function text(body, status) {
  return new Response(body, { status, headers: { "Content-Type": "text/plain" } });
}

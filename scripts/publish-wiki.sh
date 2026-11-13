#!/usr/bin/env bash
# Publish wiki/*.md (and _Sidebar.md / _Footer.md) to the GitHub wiki of
# St0pfen/z2m-guard.
#
# Why this script exists: the GitHub API has no endpoint for wiki pages, so the
# only way to publish is the separate git repository <repo>.wiki.git. That
# repository only exists after the wiki has been enabled AND a first page was
# created in the web UI — and wikis cannot be enabled at all on a private
# repository unless the account has GitHub Pro/Team/Enterprise:
#
#   PATCH /repos/<owner>/<repo>  {"has_wiki": true}   -> 200, has_wiki stays false
#   git clone <repo>.wiki.git                        -> repository not found
#
# So: run this once the repository is public (or the account is Pro). The wiki
# sources live in the repo so they are reviewed and versioned like the code.
set -euo pipefail

REPO_SLUG="${REPO_SLUG:-St0pfen/z2m-guard}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/wiki"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

[[ -d "$SRC" ]] || { echo "no wiki/ directory in $ROOT" >&2; exit 1; }

echo "== checking wiki status of $REPO_SLUG"
python3 - "$REPO_SLUG" <<'PY'
import json, re, sys, urllib.request
slug = sys.argv[1]
tok = re.match(r'https://[^:]+:([^@]+)@',
               open('/home/steffen/.git-credentials').read()).group(1)
req = urllib.request.Request(f"https://api.github.com/repos/{slug}",
      headers={"Authorization": "token " + tok, "User-Agent": "publish-wiki"})
d = json.load(urllib.request.urlopen(req, timeout=10))
if not d["has_wiki"]:
    sys.exit(f"has_wiki is false (private={d['private']}). "
             "A wiki needs a public repo or a Pro/Team/Enterprise account.")
print(f"has_wiki ok (public={not d['private']})")
PY

echo "== cloning $REPO_SLUG.wiki.git"
if ! git clone -q "https://github.com/$REPO_SLUG.wiki.git" "$WORK/wiki" 2>/dev/null; then
  cat >&2 <<'MSG'
Could not clone the wiki repository. It does not exist until the first page has
been created in the web UI (Repository → Wiki → Create the first page). Do that
once, then re-run this script — from then on the wiki is updated only from here.
MSG
  exit 1
fi

cp "$SRC"/*.md "$WORK/wiki/"
# The installation page is generated from docs/INSTALL.md so the wiki cannot
# drift away from the file reviewers see in the repository.
cp "$ROOT/docs/INSTALL.md" "$WORK/wiki/Installation.md"
cd "$WORK/wiki"
git add -A
if git diff --cached --quiet; then
  echo "== wiki already up to date"
  exit 0
fi
git -c user.name="St0pfen" \
    -c user.email="53300373+St0pfen@users.noreply.github.com" \
    commit -q -m "Sync wiki from wiki/ in the repository"
git push -q origin HEAD
echo "== published: https://github.com/$REPO_SLUG/wiki"
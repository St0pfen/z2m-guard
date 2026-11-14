# Go-public checklist

State today: the repository is **private**. Everything below is prepared and
verified up to the point where it needs a public repository — the remaining
actions are ordered so that nothing looks broken to a first visitor.

Why it is one switch, not a series of surprises: a private repo has no wiki, no
public container package and no announcement venue. Measured, not assumed:

- `./scripts/publish-wiki.sh` stops with
  `has_wiki is false (private=True). A wiki needs a public repo or a Pro/Team/Enterprise account.`
  — GitHub answers `PATCH {"has_wiki": true}` with HTTP 200 on a private repo and
  silently keeps the flag false, so the script checks the flag *and* whether
  `<repo>.wiki.git` exists instead of trusting the 200.
- `GET /user/packages/container/z2m-guard` returns **403**
  (`You need at least read:packages scope`) and `PATCH …/visibility` returns
  **404** with the current token (`repo, workflow, gist, read:org`). The package
  visibility is therefore **not** flippable from here without re-authorising with
  `read:packages` + `write:packages`.

## 1. Before the switch (done)

- [x] `v0.1.1` + `v0.1.2` released with binaries, SBOM, licence texts and `SHA256SUMS`
- [x] CI green on `test`, `msrv` and `image`; tag pipeline publishes the image
- [x] reference host runs the released version (`--version`, `/healthz`, discovery
      `sw_version` read back from the broker)
- [x] branches pruned, `delete_branch_on_merge` enabled
- [x] `docs/ANNOUNCEMENT.md` drafted; the header still says "not posted"

## 2. The switch

```bash
# repository visibility — this is the irreversible step (history becomes public)
curl -s -X PATCH -H "Authorization: token $T" \
  -H "Content-Type: application/json" \
  -d '{"private": false}' https://api.github.com/repos/St0pfen/z2m-guard
```

Then, in this order, because each step's result is the next one's input:

1. **Wiki.** `./scripts/publish-wiki.sh` — now it can create `<repo>.wiki.git`.
   The installation page is generated from `docs/INSTALL.md` at publish time, so
   it cannot drift from the reviewed file.
2. **Package visibility + link.** Needs `read:packages`/`write:packages`:
   ```bash
   gh auth login --hostname github.com --git-protocol https \
     --scopes "repo,workflow,read:org,gist,read:packages,write:packages" --web
   ```
   then either the API (`PATCH /user/packages/container/z2m-guard {"visibility":"public"}`)
   or the package page → *Package settings* → *Change visibility* → *Connect
   repository*. Without the link the package shows no README and no provenance.
3. **Anonymous pull proof** — in a shell that is logged out:
   ```bash
   docker logout ghcr.io && docker pull ghcr.io/st0pfen/z2m-guard:0.1.2
   ```
   A logged-in pull proves nothing; the visitor has no credentials.
4. **README sweep.** The links that only resolve in public (`releases`, `wiki`,
   issue templates, `ghcr.io` quickstart), the status line, and the badge — then
   open the repo page in a private browser window and click every link in the
   first screen.
5. **Announcement.** Post `docs/ANNOUNCEMENT.md` (r/homeassistant, Zigbee2MQTT
   discussions). Strip the "repository is still private" header first. Expect the
   questions it already asks for: which automation points at the marker, and
   whether 15 s grace fits a larger mesh.

## 3. Immediately after

- Watch the issue tracker for the first 48 h: the bug template asks for version,
   Z2M version and MQTT protocol because those three answer most reports.
- Tag-driven releases and `dependabot` are already automated; nothing to babysit.
- If the package stays private by accident, the README quickstart fails for
  everyone and the failure looks like a broken project rather than a settings
  mistake — check step 3 first when a user reports `unauthorized`.
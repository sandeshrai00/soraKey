# Publish to Omarchy Plugins — do this later

URL-only for now: `omarchy plugin add https://github.com/sandeshrai00/soraKey.git --enable`
No marketplace listing needed to install/update.

When ready to publish to https://plugins.omarchy.org:

1. Repo already fits `publish.html`: public GitHub repo, `manifest.json` at root, README + LICENSE, safe install/remove
2. Validate: `omarchy plugin validate ~/.config/omarchy/plugins/io.github.sandeshrai00.sorakey` must pass
3. Push code, then the ONE version tag by name (`git push origin main`, `git tag vX.Y.Z`, `git push origin vX.Y.Z` — never `--tags`: it resurrects stale local tags and fires CI on old commits). `manifest.json:version` is the marketplace display.
4. Submit: https://github.com/omacom/omarchy-plugin-marketplace/issues/new?template=submit-plugin.yml
   - repo: https://github.com/sandeshrai00/soraKey
   - category/tags as needed
5. Wait for automated validation + maintainer approval

Updates after listing: just `git push` new commits/tags — users run `omarchy plugin update io.github.sandeshrai00.sorakey --yes` (no re-publish).

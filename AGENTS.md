# Repository Instructions

## Release Flow

Keep release operations in this file, not in `README.md`. The README should stay
focused on user-facing CLI and skill installation docs.

### Version bump

1. Update the version in:
   - `Cargo.toml`
   - `package.json`
   - `Cargo.lock` after Cargo refreshes it
   - the `workflow_dispatch` tag example in `.github/workflows/release.yml`
2. Use a `vX.Y.Z` tag name. The package version is `X.Y.Z`.
3. Verify the version/tag match before tagging:

```sh
scripts/verify_release_version.sh vX.Y.Z
```

If `Cargo.lock` still contains the old package version, run `cargo check` once
without `--locked`, then rerun the locked checks below.

### Local checks

Run these before pushing the release commit and tag:

```sh
cargo check --locked
cargo fmt --check
cargo test --locked --quiet
RELATED_NPM_ALLOW_MISSING_PREBUILT=1 npm pack --dry-run
```

The `RELATED_NPM_ALLOW_MISSING_PREBUILT=1` override is only for local package
shape checks. Do not use it in CI release jobs or for a real publish.

If a Codex skill validator is available, also validate:

```sh
python3 path/to/quick_validate.py skills/find-related-files
```

### Commit and tag

```sh
git status --short
git add Cargo.toml Cargo.lock package.json .github/workflows/release.yml
git commit -m "Bump version to X.Y.Z"
git push
git tag vX.Y.Z
git push origin vX.Y.Z
```

Pushing the tag triggers `.github/workflows/release.yml`.

### Release workflow behavior

The release workflow builds and packages all supported prebuilt binaries:

- `aarch64-apple-darwin`
- `x86_64-apple-darwin`
- `aarch64-unknown-linux-musl`
- `x86_64-unknown-linux-musl`
- `aarch64-pc-windows-msvc`
- `x86_64-pc-windows-msvc`

It then uploads GitHub release assets and prepares the npm package with all
prebuilt binaries staged under `npm/prebuilt/<target-triple>/`.

The npm publish step intentionally calls:

```sh
npx -y npm@latest stage publish
```

or:

```sh
npx -y npm@latest publish --access public
```

Do not replace this with the runner's plain `npm stage publish`; older bundled
npm versions may not include the `stage` command.

### GitHub Actions approval

The `npm-publish` job uses the `npm-release` environment. If that environment
requires reviewer approval, approve the pending deployment in the GitHub Actions
UI before expecting the npm step to run.

Useful checks:

```sh
gh run list --workflow Release --limit 5
gh run watch <run-id>
gh run view <run-id> --log-failed
```

If the release workflow itself was fixed after a tag already existed, rerun the
workflow from `main` while passing the existing tag:

```sh
gh workflow run release.yml --ref main -f tag=vX.Y.Z
```

The workflow definition comes from `main`, while the build still checks out the
tag passed in `tag`.

### npm release modes

`NPM_RELEASE_MODE` controls the npm publish job:

- `stage`: run `npm stage publish`; the package is not public until approved.
- `publish`: run `npm publish --access public`; this requires a trusted
  publisher relationship that allows direct publish.
- unset or any other value: skip npm publish.

The current safe default is `stage`. With staged publishing, a successful
GitHub Actions run can still leave `npm view related-cli version` on the old
version. That means the package is staged and awaiting npm approval.

Inspect staged packages with:

```sh
npx -y npm@latest stage list related-cli --json
npx -y npm@latest stage view <stage-id>
```

After inspecting the staged package, approve it with a human 2FA prompt:

```sh
npx -y npm@latest stage approve <stage-id>
```

Then confirm the public registry and install path:

```sh
npm view related-cli version versions --json
npx -y --package related-cli@latest related --version
```

The trusted publisher setup command used for this package is:

```sh
npx -y npm@latest trust github related-cli \
  --repo ifapmzadu6/related-cli \
  --file release.yml \
  --env npm-release \
  --allow-stage-publish
```

Use `--allow-publish` only if the project intentionally switches from staged
approval to direct automated publishing.

### GitHub release verification

After the workflow succeeds, verify the GitHub release and assets:

```sh
gh release view vX.Y.Z --json tagName,url,assets
```

The release should contain one `checksums.txt` file and one `.tar.gz` asset per
supported target listed above.

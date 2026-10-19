# Releasing Linger for macOS

`scripts/release-macos.sh` builds everything on a Mac in one command. It writes to `dist/`:

| File | What it is |
|---|---|
| `linger-<version>-universal.dmg` | Disk image: the app, an Applications link and the install background |
| `linger-universal.dmg` | The same disk image under the name the website links to |
| `linger-<version>-universal.zip` | The app alone |
| `linger-<version>-SHA256SUMS.txt` | SHA-256 of the three files above |
| `Linger.app` | The app itself (universal, signed, stapled) |

The app runs on macOS 11 or later, on Apple silicon and Intel. The script signs it with the
certificate "Developer ID Application: WESLEY MYRICK COLE (66W3WWYP3B)" and turns on the hardened
runtime.
Its one entitlement, microphone input for recording sermons, is explained in
`packaging/macos/entitlements.plist`.

## One-time setup

1. **Signing certificate.** `security find-identity -v -p codesigning` must list
   `Developer ID Application: WESLEY MYRICK COLE (66W3WWYP3B)`. To sign with another identity, set
   `SIGN_IDENTITY="…"`.
2. **Tools.** Xcode or its command line tools, rustup, and [uv](https://docs.astral.sh/uv/) or
   Python 3.10+. The script adds the two Rust targets. It installs dmgbuild, the tool that lays out
   the DMG, into `target/dmgbuild`, pinned by hash in `packaging/macos/dmgbuild-requirements.txt`.
3. **Notarization credentials.** Apple must notarize the app, or other Macs refuse to open it. The
   script uses a notarytool profile named `bible-notary` from the login keychain. Create it once;
   notarytool prompts for the secret and stores it in the keychain, never in the repository.

   With an Apple ID and an app-specific password, which you create at https://account.apple.com
   under Sign-In and Security:

   ```sh
   xcrun notarytool store-credentials bible-notary --apple-id <apple-id-email> --team-id 66W3WWYP3B
   ```

   Or with an App Store Connect API key with the Developer role, made under Users and Access,
   Integrations, Team Keys:

   ```sh
   xcrun notarytool store-credentials bible-notary --key AuthKey_<KEYID>.p8 --key-id <KEYID> --issuer <issuer-id>
   ```

   Without the profile, `scripts/release-macos.sh` stops before building and prints these commands.

## Making a release

1. Set the new `version` under `[workspace.package]` in `Cargo.toml`, merge it to `main`, and
   check out a clean `main`.
2. Run `scripts/release-macos.sh`. It builds the universal app, checks its architectures, minimum
   macOS version, Info.plist and signature, runs the signed binary, notarizes and staples the app,
   builds and signs the DMG, notarizes and staples it, zips the app and writes the checksums. It
   ends by asking Gatekeeper about the app and the DMG; both must say
   `source=Notarized Developer ID`. Expect about two minutes, plus a few minutes for each of
   Apple's two notarization checks.
3. Tag the commit and push the tag:

   ```sh
   git tag -a v<version> -m "linger <version>"
   git push origin v<version>
   ```

4. Create the release as a draft, check it, then publish it on GitHub:

   ```sh
   gh release create v<version> --draft --verify-tag --generate-notes --title "linger <version>" \
     dist/linger-<version>-universal.dmg dist/linger-universal.dmg \
     dist/linger-<version>-universal.zip dist/linger-<version>-SHA256SUMS.txt
   ```

The website's Download button redirects to
`https://github.com/wesleycole/bible-rs/releases/latest/download/linger-universal.dmg`, which
GitHub serves from the newest published release. Every release must therefore include
`linger-universal.dmg`, and the site switches to a new version the moment you publish it.

To replace a published release's files, build the same version, upload with `--clobber`, then
edit the release notes:

```sh
gh release upload v<version> dist/linger-universal.dmg dist/linger-<version>-universal.dmg \
  dist/linger-<version>-universal.zip dist/linger-<version>-SHA256SUMS.txt --clobber
```

Other options:

- `scripts/release-macos.sh --skip-notarize` builds signed files without notarizing. They open on
  this Mac but Gatekeeper blocks them on others; use it to test the build.
- `scripts/release-macos.sh --ad-hoc` signs without a Developer ID. CI uses it on every push.
- `ALLOW_DIRTY=1` builds from a working tree with uncommitted changes.

## GitHub Actions

`.github/workflows/release.yml` runs the same script when someone pushes a `v*` tag, then
attaches the files to a draft release. Publishing stays manual. If a draft for the tag already
exists, the workflow replaces its files; it never touches a published release. Build locally
while GitHub Actions billing is broken. The workflow needs these repository secrets.
`gh secret set NAME` prompts for each value, so it stays out of shell history.

| Secret | Value |
|---|---|
| `DEVELOPER_ID_P12` | The Developer ID Application certificate and private key, exported from Keychain Access as a .p12, then base64: `base64 -i developer-id.p12 \| gh secret set DEVELOPER_ID_P12` |
| `DEVELOPER_ID_P12_PASSWORD` | The password chosen during that export |
| `NOTARY_API_KEY` | The contents of the App Store Connect API key file: `gh secret set NOTARY_API_KEY < AuthKey_<KEYID>.p8` |
| `NOTARY_API_KEY_ID` | That key's ID |
| `NOTARY_API_ISSUER` | The issuer ID shown above the list of keys |

Delete the exported .p12 once the secret is set. The workflow imports the certificate into a
temporary keychain and deletes that keychain when it finishes.

## Notes

- **Keychain.** API keys stay in the login Keychain. Each item trusts the app's designated
  requirement (bundle ID `com.wesleycole.linger.mac`, team 66W3WWYP3B), so every release signed this
  way reads them without a prompt. Changing the bundle ID or the signing team makes every user approve
  access once. A key saved by an ad-hoc signed development build asks for approval once, the first
  time a release build reads it.
- **Updates.** Settings › About › Check for Updates… opens the latest release page in the browser.
  The app makes no network requests of its own to check for updates.
- **Rejected notarization.** `scripts/notarize-macos.sh` prints Apple's log, which names each
  file that failed and why.
- **Intel testing.** On an Apple silicon Mac without Rosetta, the script checks the x86_64 slice's
  architecture and minimum macOS version but cannot run it. `softwareupdate --install-rosetta`
  lets it run that slice too.

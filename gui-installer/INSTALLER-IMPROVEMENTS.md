# Installer Improvements (Windows)

A backlog for `gui-installer/sinorag-installer`, written from a Linux dev box —
none of this is compiled or tested here, so treat every item as a recommendation
to verify on Windows. Ordered roughly by user-visible impact.

The installer today is in good shape: it installs under `%LOCALAPPDATA%\Programs`
so it needs no UAC elevation, it version-gates and fingerprints an existing
install before redoing work (`existing_install_summary`), and it maps download +
index-build progress onto the bar cleanly. The items below are the gaps.

---

## 1. Code signing / SmartScreen — without paying

The unsigned `sinorag-installer.exe`, when downloaded, trips **Microsoft Defender
SmartScreen** ("Windows protected your PC — unknown publisher"). Two separate
mechanisms are in play, and it's worth not conflating them:

- **Authenticode trust** — "is the publisher's signature valid and trusted?"
  Fixed by signing with a certificate chained to a root the machine trusts.
- **SmartScreen reputation** — "have lots of people run this exact binary
  safely?" Reputation accrues per-binary/per-cert over downloads. A *brand-new*
  signature has no reputation and can still warn until it builds up (an EV cert
  gets instant reputation, but EV certs cost money).

So signing helps with "unknown publisher" but doesn't, by itself, make a niche
tool's first downloads warning-free. Given you've ruled out paid certs, the
realistic free options:

### Option A (recommended, zero cost): own the "Run anyway" flow + checksums
Don't sign. Instead:
- Publish a **SHA-256 checksum** next to the installer download and a one-line
  verify command (`Get-FileHash .\sinorag-installer.exe`). This gives security-
  conscious users integrity verification, which is what signing's *integrity*
  half provides anyway.
- Add a short **"Windows will warn you — here's why and how"** section to the
  download docs with the exact `More info → Run anyway` click path and a
  screenshot.
- This is honest for an open-source, not-widely-marketed tool and costs nothing.

### Option B: SignPath.io Foundation (free for OSS — the one real free *trusted* cert)
SignPath offers free code-signing certificates to qualifying open-source
projects. Caveats: the project must be public and meet their eligibility rules,
and signing happens in **CI** (you upload the artifact from a GitHub Actions /
CI build to their signing service). Worth it only once releases are built in CI
rather than by hand. Removes "unknown publisher"; SmartScreen reputation still
builds over time.

### Option C: self-signed certificate
`New-SelfSignedCertificate` + `signtool sign /fd sha256`. **Only useful if users
import your `.cer` into Trusted Root + Trusted Publishers first** — otherwise the
signature is "not trusted" and you're no better than unsigned. Fine for a known
internal audience, pointless for public download. Generally skip.

### Option D: distribute via `winget`
Submitting a manifest to the winget community repo means users run
`winget install SinoRAG`, which carries its own trust surface and lets
reputation accrue to the package. Doesn't remove the need to sign for direct
`.exe` downloads, but it's a friendlier primary install path on modern Windows.

**Suggested plan:** Option A now (free, immediate), Option B when releases move
to CI, consider D as the advertised install path. Never pay for C/EV here.

---

## 2. OpenCode bootstrap is fragile on a clean Windows

`install_or_verify_opencode` tries, in order:
1. `curl.exe ... | bash` — **requires `bash.exe`** (Git Bash / WSL), absent on a
   fresh Windows, so this path is skipped on exactly the machines that need it.
2. `npm install -g opencode-ai` — **requires Node.js**, also commonly absent.

A clean machine has neither, so the install silently degrades to "OpenCode not
found." Improvements:
- Prefer **`winget install` for OpenCode** if a winget package exists — winget
  ships on Windows 10 21H1+ and needs no bash/node.
- Or **bundle a pinned OpenCode binary** in the payload and drop it next to
  `sinorag.exe`, removing the network/runtime dependency entirely.
- At minimum, make the missing-OpenCode log line a **clickable download URL** and
  surface a clearer post-install instruction in the Complete state.

---

## 3. No uninstaller / "Apps & features" entry

Nothing registers the install, so it can't be removed from Settings → Apps. Add
an uninstall registry key on install:

```
HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\SinoRAG
  DisplayName     = "SinoRAG <version>"
  DisplayVersion  = "<version>"
  DisplayIcon     = "<install>\SinoRAG.ico"
  Publisher       = "SinoRAG"
  InstallLocation = "<install>"
  UninstallString = "<install>\sinorag.exe uninstall"   (or a small uninstaller)
  NoModify = 1, NoRepair = 1
```

(HKCU because the install is per-user under `%LOCALAPPDATA%`.) This implies a
small `sinorag uninstall` subcommand or a separate uninstaller exe that removes
the install dir, shortcuts, and this key.

---

## 4. Smaller correctness / robustness items

- **`CoInitialize` without `CoUninitialize`** (`create_shortcuts`). Harmless
  since the process exits, but pair them for correctness, or use
  `CoInitializeEx(COINIT_APARTMENTTHREADED)` scoped to the shortcut work.
- **`which()` reimplements PATH search** without honouring `PATHEXT`. It works
  only because callers pass explicit `.exe`/`.cmd`/no-ext variants. The `which`
  crate handles PATHEXT and is a common, small dependency — consider swapping.
- **Single-instance guard.** Two installer windows racing the same target dir is
  possible; a named mutex (`CreateMutexW`) keeps it to one.
- **Long-path / spaces in install path.** `validate_install_path` checks parent
  writability but not the 260-char `MAX_PATH` ceiling; a deep default + corpus
  tree could exceed it on systems without long paths enabled. Worth a check or a
  note.
- **Self-extracting payload in memory.** `PAYLOAD_7Z` is `include_bytes!` into
  the binary, so the whole archive is resident and written out before extraction.
  Acceptable for a one-shot installer; just be aware the installer exe is as
  large as the compressed payload.
- **Console window on the shortcut.** Shortcuts launch `sinorag.exe agent`; if
  `sinorag` is a console subsystem binary, a console window will appear. That's
  presumably intended for an interactive agent CLI — confirm it's the experience
  you want, or wrap with a launcher.

---

## 5. Repo hygiene — vendored `freya`

`gui-installer/freya/` checks the **entire Freya framework plus all its examples**
into the tree (hundreds of files), which bloats the repo and clones. Replace with
either a git submodule pinned to a rev, or a normal Cargo dependency
(`freya = "<version>"` from crates.io, or `git = ... rev = ...`). The installer's
`Cargo.toml` already references it by path; only the vendored source needs to go.

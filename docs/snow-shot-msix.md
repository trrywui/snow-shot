# Publish Snow Shot to Microsoft Store

The Store release contains Snow Shot Full for x64. Microsoft Store
manages installation, updates and removal. Standalone NSIS and portable releases
remain available through their existing release process.

## Before you build

Verify the reserved product's identity in Partner Center against
`snow_shot/packaging/msix/store-identity.json`:

| Field | Value |
| --- | --- |
| Package name | `SnowApps.SnowShot` |
| Publisher | `CN=E1F3693C-C740-4D31-B5DA-7520890E22DC` |
| Publisher display name | `Snow Apps` |
| Package family | `SnowApps.SnowShot_3pp9w6ncw7bs0` |
| Store product ID | `9NRTJ1GBX69V` |

Keep this identity across releases. A change also affects startup, MCP and access
to existing Store data.

Use PowerShell 7.2+, the repository's Windows build toolchain, static Qt and
vcpkg dependencies, and Windows SDK 10.0.26100.0 or newer with MakeAppx, MakePri
and SignTool.

If the dependency audit reports an outdated installation, stop builds using that
installation and refresh it through bootstrap:

```powershell
pwsh -File scripts/bootstrap.ps1 -Architecture x64 -Preset snow-shot-msvc-store-release -VcpkgVariants Static
```

Rebuild afterward without `-SkipBuild`.

Before submission, review the [English privacy policy](snow-shot-privacy-policy.en.md),
[Chinese privacy policy](snow-shot-privacy-policy.zh-CN.md), and internal
[publisher review appendix](snow-shot-privacy-publisher-review.md). Confirm the
outstanding deployment facts and approve the public text. Publish it through the
website project at `D:\snow-apps-site`, then verify the live page at
`https://snowshot.top/privacy.html`. Local package and website builds do not confirm
live publication or how the deployed service handles data.

## Build the submission package

Stable x64 release packaging through `scripts/package-snow-shot.ps1` (including
`stage-snow-shot-release.ps1` and the local release publisher) also builds the Store
preset and generates an unsigned MSIX. It reuses the verified x64 OCR runtime and
saves the package, SHA-256 sidecar and `store-packages.manifest.json` under
`artifacts/msix/<version>/`. These files stay local for a later manual Store
submission; the publisher does not upload them to GitHub, Gitee or Microsoft Store.
ARM64 and preview releases keep their existing standalone outputs.

With `package-snow-shot.ps1 -SkipBuild`, both the standalone and Store build trees
must already contain the same release version. The Store build uses
`build/snow-shot-msvc-store-release` even when a custom standalone build directory
is selected. Staging-only operations do not generate another MSIX.

To generate only the Store package, run from the repository root:

```powershell
pwsh -File scripts/package-snow-shot-msix.ps1
```

The command builds the x64 Store preset and writes an unsigned `.msix` package,
a SHA-256 sidecar and `store-packages.manifest.json` to `artifacts/msix/packages`.
The package includes English, Simplified Chinese and Traditional Chinese resources.

| Package | Minimum Windows version |
| --- | --- |
| x64 MSIX | Windows 10, build 19041 |

The default architecture is x64; `-Architecture x64` also selects it explicitly.
After a successful Store build, `-SkipBuild` reuses it while still staging and
auditing the package:

```powershell
pwsh -File scripts/package-snow-shot-msix.ps1 -Architecture x64 -SkipBuild
```

The build tree is `build/snow-shot-msvc-store-release`. A standalone build cannot
be reused for Store packaging. Each packaging run replaces the evidence manifest
in its output folder; use separate folders when keeping results from separate runs.

The Store package includes the pinned OCR worker and Small model for offline use.
Other OCR models can be downloaded as data; executable updates come through Store.
If supplying a previously prepared OCR archive, use `-OcrRuntimeArchives` with an
`x64` entry in the architecture-to-path hashtable. It must match the trusted release
manifest; packaging does not replace its pins.

## Sign a local test package

The default package is an unsigned submission file. Microsoft Store signs it
before delivery; it cannot be installed directly from the output folder.
For local testing, create a separately signed copy and explicitly trust its test
certificate on the test PC.

In your normal PowerShell session at the repository root, create a short-lived
code-signing certificate with the matching Publisher. Its private key stays
nonexportable in your user certificate store; only the public `.cer` is exported:

```powershell
$identity = Get-Content snow_shot/packaging/msix/store-identity.json -Raw |
  ConvertFrom-Json
$testOutput = Join-Path $PWD 'artifacts/msix/local-test-x64'
$null = New-Item -ItemType Directory -Path $testOutput -Force
$certificate = New-SelfSignedCertificate -Type Custom -Subject $identity.Publisher `
  -CertStoreLocation Cert:\CurrentUser\My -KeyUsage DigitalSignature `
  -KeyAlgorithm RSA -KeyLength 2048 -HashAlgorithm SHA256 `
  -KeyExportPolicy NonExportable -NotAfter (Get-Date).AddDays(90) `
  -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3', '2.5.29.19={text}')
Export-Certificate -Cert $certificate `
  -FilePath (Join-Path $testOutput 'SnowShot-LocalTest.cer')
$certificate.Thumbprint
```

If you already have a suitable certificate, reuse it and export its public `.cer`
instead. Record its thumbprint. Do not export or distribute its private key.

An administrator must now import that public certificate into the test PC's
**Local Machine → Trusted People** store. Open an administrator PowerShell session
at the repository root, confirm the certificate's thumbprint, and run:

```powershell
Import-Certificate -FilePath .\artifacts\msix\local-test-x64\SnowShot-LocalTest.cer `
  -CertStoreLocation Cert:\LocalMachine\TrustedPeople
```

This is an explicit machine-wide trust decision. Importing into a current-user
trust store is insufficient for App Installer. If signing and testing use different
PCs, the signing PC also needs trust for the helper's verification step. See
Microsoft's [certificate trust guidance](https://learn.microsoft.com/en-us/windows/msix/app-installer/troubleshoot-appinstaller-issues#trusted-certificates).

Back in your original, normal-user PowerShell session, package and install the
test copy. Keep its output separate from the unsigned submission files:

```powershell
pwsh -File scripts/package-snow-shot-msix.ps1 -Architecture x64 -SkipBuild `
  -TestCertificateThumbprint $certificate.Thumbprint -OutputDirectory $testOutput
if ($LASTEXITCODE -ne 0) { throw 'Local test packaging failed.' }
$testEvidence = Get-Content (Join-Path $testOutput 'store-packages.manifest.json') -Raw |
  ConvertFrom-Json
$testPackage = $testEvidence.Packages[0].Path
Get-AuthenticodeSignature -FilePath $testPackage |
  Format-List Status, StatusMessage, SignerCertificate
Add-AppxPackage -Path $testPackage
```

The helper signs with SHA-256 and runs `signtool verify /pa /v`. It does not create
certificates, change trust or install packages. Run the install command only after
packaging and verification succeed; the signature status should be `Valid`.

`0x800B010A` means Windows could not build a trusted certificate chain. Check the
exact file with `Get-AuthenticodeSignature`: `NotSigned` is expected for submission
output, while a signed test copy needs the matching certificate trusted on that
PC. Trusting a certificate does not add a signature to an unsigned package.

After testing, an administrator can remove this temporary trust by substituting
the recorded test-certificate thumbprint below:

```powershell
Remove-Item 'Cert:\LocalMachine\TrustedPeople\YOUR_TEST_CERTIFICATE_THUMBPRINT'
```

Microsoft's [local signing guide](https://learn.microsoft.com/en-us/windows/msix/package/sign-msix-package-guide)
explains the same signing and trust sequence.

## Validate the release

Run the focused packaging checks:

```powershell
pwsh -File scripts/test-snow-shot-msix.ps1
pwsh -File scripts/test-snow-shot-msix.ps1 -SdkSmoke
pwsh -File scripts/test-snow-shot-package-dependencies.ps1
pwsh -File scripts/test-windows-release-workflow.ps1
```

The SDK fixture checks resource generation, packing and unpacking for x64 with
synthetic executables. It does not certify Snow Shot. A release also needs the
optimized production build and audited x64 package, followed by installation
tests on real systems.

The evidence manifest records `NativeValidationRequired=true` for a cross-built
architecture whose startup/OCR probes could not run on the build host. Complete
native validation before publishing it. A false value only means those build-time
probes ran; installed-package testing is still required.

Before submission:

- Test a signed installation on clean x64 systems, including Windows 10 build
  19041 and Windows 11. Check launch, all three languages, offline Small OCR,
  shortcuts, capture, annotation, pinning and recording.
- Check denied microphone/camera access, startup enable/disable, user-disabled and
  policy-disabled startup, and Explorer's Edit action. Right-click supported image
  and text files with another app set as their default: check Edit with Snow Shot
  in Windows 10's classic menu and Windows 11's modern menu and Show more options.
  Test multiple selections, Unicode paths and paths containing spaces; unsupported
  files and folders must not offer the command. Verify the MCP alias with a
  real stdin/stdout initialization handshake from two simultaneous clients.
- Test coexistence with a standalone installation. Upgrade from a lower package
  version and verify that Store settings/data are preserved. Check quiet restart
  and uninstall.
- Keep edited pins and a recording active during an update. Check deferred shutdown,
  cancellation and next-launch recovery after both successful and interrupted
  recording finalization. Apply the recovery limitations below when assessing results.
- Run [Windows App Certification Kit](https://learn.microsoft.com/en-us/windows/uwp/debug-test-perf/windows-app-certification-kit)
  against the real package in an active administrative user session. Retain its
  XML/HTML report, package hashes, OS/build/architecture and test results.

A successful MakeAppx run does not establish WACK certification. The packaging
scripts do not run WACK, upload to Partner Center or complete publisher review.

## Store behavior and recovery limits

Startup is initially disabled. Users can enable it in Settings, subject to Windows
policy. The package declares supported file types and an Explorer Edit with Snow Shot
command without changing the user's default file associations. The native
`SnowShotExplorerCommand.dll` implements `IExplorerCommand` and is registered by
the MSIX manifest for both classic menus and the Windows 11 modern menu. Windows
manages registration and removes it when the package is uninstalled; Snow Shot
does not write runtime shell registrations. See Microsoft's
[packaged Explorer command guidance](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/integrate-packaged-app-with-file-explorer).
MCP uses the console alias
`snow-shot-store-mcp.exe`.

The Store build uses separate default settings/data locations from the standalone
app. It omits administrator launch and runtime shell-registration controls.
Microphone and camera features remain subject to Windows privacy settings.

During an update, Snow Shot can defer cooperative shutdown while work is active.
When notified of forced shutdown, it attempts to preserve work for up to four
seconds. The most recent pin edit may still be lost. Recording recovery can reopen
a finalized `.snowrec` source or reveal retained files on the next launch.
Interrupted raw staging may require manual recovery; keeping it does not guarantee
that incomplete media can be played or exported.

## Submit and roll out updates

Follow Microsoft's [MSIX submission guide](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/create-app-submission?pivots=store-installer-msix):

1. Open the reserved Snow Shot product and verify its Product identity before
   uploading the x64 package. Check the processed OS, architecture and language
   information.
2. Increase the stable app version for every replacement release. `major.minor.patch`
   becomes `major.minor.patch.0`; major must be nonzero, each component must fit
   0–65535, and prereleases are rejected. The final component stays zero for Store.
   See the [package version requirements](https://learn.microsoft.com/en-us/windows/apps/publish/publish-your-app/app-package-requirements?pivots=store-installer-msix).
3. Complete availability, age ratings, listings/screenshots, support contacts and
   the published privacy-policy URL. Describe online AI recognition/translation and
   declare live generative AI under the [AI content policy](https://learn.microsoft.com/en-us/windows/apps/publish/store-policies#1116-live-generative-ai-content).
   Confirm that privacy requests, feedback and inappropriate-content reports have
   monitored contact routes. About → Feedback and suggestions opens the project's
   issue tracker.
4. Explain the restricted `runFullTrust` capability: native screen capture, global
   shortcuts, annotation/pinning windows, local OCR and recording. Explain microphone
   and webcam use for recording. Supply reviewer instructions and any access needed
   for account-gated features, then choose publication timing and submit after the
   release checks and publisher review are complete.

Suggested reviewer notes, to confirm against the submitted build:

> Snow Shot Full is a native desktop capture, annotation, pinning and recording
> app. Core capture and bundled Small OCR work offline without an account. Launch
> from Start, capture a region, annotate/save it and pin an image. Recording has
> microphone and camera controls and supports Windows permission denial. Explorer's
> Snow Shot Edit action opens supported files. Optional official online recognition
> and AI features need a network connection; custom providers need configuration.
> About links to privacy and feedback. Settings can enable startup and authenticated
> local MCP through snow-shot-store-mcp.exe. Microsoft Store manages updates.
> Administrator launch and runtime shell-registration controls are absent.

Test updates through a private package flight before expanding production
[gradual rollout](https://learn.microsoft.com/en-us/windows/apps/publish/gradual-package-rollout).
Monitor crash/recovery reports and increase delivery as appropriate. Halting a
rollout stops further delivery; it does not downgrade devices already updated.
Use a higher version to correct those installations, and finalize or halt a
rollout before starting the next submission.

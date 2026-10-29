# Snow Shot privacy policy

Last updated: October 19, 2026

Snow Shot is operated by **Chao Huang**, publishing as **Snow Apps**. This policy
explains how the app, its official online services, and this website handle your
information.

For privacy questions or requests to access, correct, or delete information,
email **[chao@mgchao.top](mailto:chao@mgchao.top)**.

## Content on your device

Screen capture, annotation, recording, standard text recognition (OCR), and QR code
reading run on your device. Online features and connections to AI clients can send content
elsewhere, as described below.

Snow Shot works with the images, files, clipboard content, text, and annotations you
capture, open, or select. Recordings can include system audio, microphone audio,
webcam video, and keyboard or mouse overlays when you enable those features.

Editable recording source files can retain mouse positions, clicks, and timing
even when mouse effects are hidden. Keyboard key events are retained when the
keyboard overlay is enabled.

The app saves settings, screenshot history, pinned content, recordings, thumbnails,
and temporary files locally. Screenshot history may include the original screen
image, including areas outside the crop you export. Pinned history can contain
images, text, recognition results, original file names and paths, window positions,
and timestamps.

You can adjust history limits, keep selected items, clear history and caches, and
choose storage and output folders. The default limits for screenshot history and
closed pinned history are seven days, 100 entries, and 1 GiB. Open pins are saved
separately. Turning history off stops new history from being saved; it does not
delete existing items and can pause automatic cleanup. Use the clear-history
controls to remove saved items.

Images kept for clipboard use are normally cleaned up after seven days when the
app runs. Backups of damaged settings files are normally cleaned up after 30 days
when the app runs. Recording source and recovery files can remain alongside your
recordings and are outside the history limits. Remove them when you no longer need
to edit or recover the recording. Files you export or keep in custom folders remain
under your control.

## Online features and services you choose

An online feature sends the content needed for the task to the selected service.
Automatic actions you enable, such as translation, can also send requests. You can
change the service or turn these actions off in Settings.

- **Table and formula recognition:** the official service receives a prepared image
  for recognition on its private servers.
- **AI conversion and translation:** the official service forwards the relevant
  images, text, instructions, and model options to its configured AI provider.
  Alibaba Cloud Model Studio (百炼) is the provider documented for this integration.
  Provider data handling depends on the service, region, and plan used; see
  [Alibaba Cloud's privacy information](https://help.aliyun.com/zh/model-studio/privacy-notice).
- **Custom AI and translation services:** the service you configure receives your
  submitted content and the credentials needed to process it. Its own terms govern
  processing locations, retention, and use of data for model training.
- **Cloud Upload:** an exported image is sent to your chosen S3-compatible storage
  service. Depending on your settings, the uploaded file may be accessible through
  a public or CDN link. Manage or delete uploaded files through that service.
  Cancelling an upload request does not guarantee removal of a file already received.

The official API uses HTTPS. With a custom service, the connection depends on the
endpoint you configure. Before sending sensitive content to an online service,
review the selected provider's terms, including its retention and model-training
practices. Different providers and service plans can have different rules.

Downloading recognition models contacts the model host, currently ModelScope, to
retrieve files. It does not require sending your screenshots to that host.

## Information used to run the official service

The official API receives your IP address as part of a network request. It uses an
identifier derived from that address to apply usage limits and help prevent abuse.
This identifier is pseudonymous, not anonymous. The service also records request
times, the feature and model used, success or failure, and usage and accounting
totals.

The service database keeps these operational records rather than a content history
of your submitted images, prompts, translations, or recognition results. Recognition
workers process images in memory. Hosting systems, network logs, backups, and
external AI providers are separate parts of the service, so this does not mean that
every system handling a request has zero retention.

Operational records are cleaned up automatically. The default period for operations
and related records is 90 days from settlement. For usage summaries and eligible
inactive identifiers, it is 400 days. These are configurable defaults, not a
guaranteed maximum: unfinished work and records still
needed by other operations can remain longer. Logs, backups, and external providers
follow their own retention settings. Contact us if you need more information about
retention or processing locations for the official service.

## Credentials, diagnostics, and connected AI clients

Custom service keys and cloud-storage credentials are kept in local configuration
files. These files and regular settings exports are not encrypted and can contain
your credentials. Treat them and any backups as sensitive. Settings exported
through MCP hide credential fields.

Snow Shot keeps diagnostic logs and crash reports on your device to help diagnose
problems. They can include app and session details, timings, image dimensions,
model identifiers, and errors. The app tries to remove secrets and sensitive path
or URL details, but a diagnostic archive or crash dump can still contain sensitive
information, including data from app memory. Automatic crash-report uploads are
disabled. You choose whether to export and share a diagnostic archive. The official
online service also uses logs, metrics, and traces to maintain reliability and
investigate problems.

MCP, which lets an AI client work with Snow Shot, is off by default. When you enable
it, an authenticated local connection can give that client access to supported
captures, files, recognized content, settings, and app actions. The client may pass
returned content to its own AI service. Review that client's settings and privacy
terms, and disable MCP or disconnect it when you no longer need the connection.

## This website and app updates

The website uses browser storage to remember your chosen language and appearance.
Optional language troubleshooting information is kept in your browser session.
You can remove these preferences by clearing this site's browser data. The website
loads fonts from Google Fonts, which receives the network information needed to
serve those files. Links to downloads, source code, and community pages take you
to the relevant external service.

Microsoft Store installations receive app updates through Microsoft Store.
Depending on your settings, standalone installations can automatically check for
and download updates from the selected release service, currently GitHub or Gitee.
Model hosts, release services, and website hosting systems receive the network
information needed to serve your
requests and may keep their own access logs.

## Your choices and privacy requests

You can stop using online features, remove service credentials and upload
destinations, turn off MCP, clear local history, and delete files you have saved.
Uninstalling the Microsoft Store app normally removes its package-managed data.
Files in custom folders, exported recordings or images, and remote uploads need
to be managed separately.

Clearing local history does not delete official-service records or third-party
uploads. To ask about information held by the official service, or request access,
correction, or deletion, email **[chao@mgchao.top](mailto:chao@mgchao.top)**. Include
enough information to help us locate the relevant records. We may need to verify
your connection to those records; please do not send unrelated sensitive captures
or service passwords.

We will update this policy when the way Snow Shot handles information changes.
The date at the top identifies the latest revision.

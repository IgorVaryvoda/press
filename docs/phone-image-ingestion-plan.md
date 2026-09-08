# Phone-image ingestion: a bounded HEIC/HEIF plan

Proposed investigation and implementation, 2026-09-08. Owner role: processing
engineer with packaging support. Promote this work when consented pilot folders
show unsupported phone images blocking useful jobs; no decoder is selected here.

## Why this is different from RAW development

Current `src/scan.rs` and the README count HEIC/HEIF inputs as skipped rather than
listing decoded images. The honest skip message should stay until support is
real. Apple's [HEIF/HEVC guidance](https://support.apple.com/en-us/116944), checked
2026-09-08, documents HEIF capture and preservation in Apple workflows. That is
an input-compatibility reason to investigate support, not evidence that every
supplier shoots on an iPhone or that all HEIF variants are equivalent.

The first goal is to open ordinary still product photographs and prepare supported
deliverables. No RAW development, Photos-library catalog, video editor, Live Photo
motion processing or tethering stack. Never silently reinterpret an unsupported
HDR/multi-image container as an ordinary SDR still.

## I1: samples and implementation decision

Collect consented examples with capture/export context and explicit retention
permission; prefer redistributable synthetic fixtures for the public repository.
Record frequency, affected jobs and current workarounds rather than guessing value.

Compare platform-native decoding and a maintained cross-platform decoder on the
same corpus. Record license/redistribution review, codec/patent assessment by the
appropriate owner, shipped dependency size, supported platforms, update/security
maintenance and memory behavior. Open-source availability alone does not settle
all redistribution questions. Do not add a dependency or promise platform parity
until that decision has an accountable owner and installation evidence.

## I2: one truthful decode path

Integrate at the existing scan/decode/preparation boundary so thumbnails, comparison,
estimation, conversion and optional prepared Studio uploads agree. Recognize the
container from its bytes; test mislabeled input. Header dimensions and the rendered
image must refer to the chosen primary image, not an embedded thumbnail.

Define orientation, color profile, sample depth and alpha handling explicitly.
For 10-bit/HDR/gain-map or multi-image variants, either implement and test the
intended interpretation or refuse with a named reason. Any tone mapping or depth
reduction must be visible and separately supported; it cannot masquerade as
lossless preservation. Retain existing high-depth lossless refusal rules.

Keep local source files unchanged. Give malformed/oversized inputs bounded resource
handling. A missing runtime or unsupported platform names the limitation and leaves
other files usable; it must not silently send an image to a hosted converter.
A user-authorized cloud operation remains a separate action under the existing
Studio contract.

## I3: packaging and release proof

| Fixture or journey | Required evidence |
| --- | --- |
| Ordinary primary still | Correct dimensions/orientation and the same selected image across all local entry points |
| Tagged color, grayscale, transparency, high depth | Preservation or disclosed transformation; named refusal where unsupported |
| HDR/gain-map/multi-image content | Explicit supported interpretation or refusal, never silent flattening/tone mapping |
| Misnamed, truncated, corrupt or oversized file | Content detection, useful error and bounded resource behavior |
| Mixed folder | Good JPEG/PNG files still work; failures identify the unsupported phone files |
| Fresh supported OS installation, then offline use | Required decoder is packaged or its installation is explicit; local work does not require cloud access |

Use expected pixels/metadata and a trusted reference decoder where appropriate,
not screenshot appearance alone. Run Linux, macOS and Windows packaging checks
for each platform being advertised; a source build on one OS proves no other OS.
Do not bundle model downloads merely to decode an input image.

Ship only the demonstrated still-image subset and publish a format/variant support
matrix. If a regression appears, disable that decoder path while preserving source
files, old outputs and explicit skip/refusal messages. Broader HEIF support is a
follow-up decision, not a hidden requirement of the first release.

# Screenshot provenance

Captured from the shipping Press 0.6.6 macOS app on 8 September 2026. The source
checkout was `8d2462a`. These are real application captures, without compositing
or substituted controls. The audit, gallery, comparison and saved-results views
use the same six PNG exports in a disposable `Product photos` folder.

The sample images are decoded and resized derivatives of Sirv’s public
[navy-shirt front photo](https://demo.sirv.com/demo/sirv-media-viewer/ralphlauren/featherweight-shirt-navy-1.jpg)
and [second photo](https://demo.sirv.com/demo/sirv-media-viewer/ralphlauren/featherweight-shirt-navy-2.jpg).
Photography belongs to the original rights holders and is used as Sirv demo
material. These PNG exports are not camera originals; their displayed savings
illustrate this sample batch, not a general compression benchmark.

The comparison uses WebP quality 80 at the original dimensions. The results view
shows the six files actually written under `optimized/`, with originals intact.

The captures are 1170 × 768 pixels. All four PNG captures together were 436 KB;
Press encoded them to 236 KB of WebP with:

```sh
press convert /tmp/press-screenshots --quality 90 --max-edge 1440
```

The size cap did not upscale the captures. These same WebP files are used on the
ImageGuide Press page and the Press project and build record on varyvoda.com.

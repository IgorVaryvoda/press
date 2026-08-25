# Soul

What Press is, who it serves, and which arguments are already settled. Read this
before proposing a feature. If a proposal fights a belief below, the proposal
loses — or the belief gets rewritten here first, not quietly in code.

## The name

**Press.** Inside the Sirv family it is **Sirv Press**. The short name stands on
its own, so the product does not depend on the parent brand to make sense.

Press means two true things at once:

- to press is to compress, which is the core act;
- prepress is the check that makes a file fit to ship, which is the end state.

It also reads as an instruction on the command line: `press ~/photos --convert`.

imageguide.dev and the Chrome extension keep the ImageGuide name. They audit and
report. Press is the tool that rewrites your files, so it earns its own name.

## What it is

The desk between the shoot and the store.

A folder of images sits on your disk. Press tells you what is really in it, what
it weighs, whether it is fit to ship, and whether the copy on Sirv matches. Then
it does the work: convert, fix, generate, and push. Local by default, cloud when
the cloud is genuinely better.

Lightroom judges how an image looks. Press judges whether it is fit to ship, and
then ships it.

## Who it serves

The person holding 5,733 files and a deadline. A product photographer, an
e-commerce team, a developer who inherited `public/images`. They are not grading
colour. They need to know which files are wrong, which are heavy, which are
missing, and which will fail the marketplace check — before the client asks.

They are not looking for a creative tool. They are looking for the end of a task.

## Beliefs

These decide arguments.

**The file is the truth, not its name.** Formats come from magic bytes. The first
folder this ran on held 169 files named `.webp`; 59 were PNG. A column that
disagrees with the extension is a finding, not a bug.

**Your machine is enough.** Auditing, comparing, and converting never touch the
network. The folder is bounded by the disk, not by a browser tab. Bytes leave the
machine only when the user asks, by name, with a button.

**Show the trade, do not score it.** At q40 a 12 MB photo becomes 262 KB and the
sky goes from grainy to smooth. Whether that is acceptable is a judgement. Put
both images on screen, registered, and let the person decide. A quality score
would be a smaller feature and a worse one.

**Heaviest first.** The list is sorted by where the work is. Nobody needs the
20 KB thumbnails at the top.

**Never lie by omission.** Camera raw is counted and named as skipped, not
silently dropped from the total. Failures name the first few files, because
"3 failed" is not a report. A stale saving next to a new file is a lie, so
changing folders drops every old result.

**Explicit beats automatic.** Push is a button. Studio opens only a byte-identical
image, and says to push first when it is not. Press never repairs a mismatch by
guessing which side the user meant.

**Speed is correctness at this scale.** A 6,000 image folder does not decode 6,000
files. Thumbnails decode off the main thread, only when the viewport asks. There is
no subprocess per image. An audit that takes a minute does not get run.

**Boring and finished beats clever and pending.** No generic provider abstraction,
no workflow engine, no plugin system before the second real user of it exists.

## Where it goes

Four layers. Each one earns the next; none of them get pre-built.

1. **Truth** — what each file is, weighs, and costs. Local. Shipped.
2. **Bridge** — the local folder and its Sirv folder as one view. Push, pull, and
   handoff into Sirv AI Studio. Shipped thin.
3. **Ops** — background removal, upscale, generation, tagging. Local models do the
   deterministic, repetitive work for free and offline. Studio does the heavy
   generative work. Press picks the venue and says which one it used.
4. **Fitness** — the marketplace pre-flight. Aspect ratio, dimensions, background,
   transparency, weight, naming, missing views. Provenance travels with every
   derivative: source path, model, prompt, approval state.

The split in layer 3 is the product. Studio charges credits per operation. A
machine with a GPU does the boring 80 percent for nothing, offline, across the
whole folder at once. The cloud does the 20 percent that needs a large model.
Neither side alone is the answer, which is why a desktop app exists at all.

## What it is not

- Not an editor. Nothing here competes with an adjustment brush.
- Not a DAM. It reads your folder; it does not want to own it.
- Not a cloud service with a desktop skin. Offline is the normal case.
- Not a Sirv upsell. Sirv sync is optional, and Press is honest with an unpaired
  folder.
- Not a general batch runner. Every operation exists because a delivery decision
  needed it.

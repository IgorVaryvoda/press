# Check a local requirements snapshot

Use a bounded, local, user-authored requirements file to inspect actual output
bytes:

```bash
press check <file-or-folder> --requirements-file <local-spec> --json
```

The report uses relative names and evidence from the same bounded input bytes:
content-derived format, dimensions, byte counts and hashes. Required checks that
the supported engine cannot perform remain `not_checked` and prevent an
all-required-pass result. This command does not contact a retailer, issue
approval, read signed URLs or use a remote policy feed.

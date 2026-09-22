# Pinky vault tutorial

This hands-on tutorial uses the fictional **Project Alder** files in
[`examples/pinky-tutorial-sources`](../examples/pinky-tutorial-sources). Nothing
in the example pack describes a real person, site, or incident.

You will retain several supported file types, search across them, inspect exact
citations, and observe Pinky create a new source version after an edit. The
exercise takes about 15 minutes once Pinky is running with an unlocked vault.

## Before you begin

Complete the [vault setup guide](VAULT_GUIDE.md) and confirm that Pinky shows
**Vault unlocked**. The tutorial exercises current lexical retrieval; it does
not require or demonstrate conversational AI.

Make a working copy so that the file-watching exercise does not modify your Git
checkout:

```bash
mkdir -p "$HOME/Documents"
cp -R examples/pinky-tutorial-sources \
  "$HOME/Documents/pinky-tutorial-sources"
```

Run that command from the Pinky repository root. If the destination already
exists, choose a new destination name instead of mixing the packs. For the
steps below, the approved directory is:

```text
/home/you/Documents/pinky-tutorial-sources
```

Replace `/home/you` with your actual home path. You can obtain the exact path
with:

```bash
realpath -- "$HOME/Documents/pinky-tutorial-sources"
```

## Lesson 1: Retain the source pack

Pinky currently adds one file at a time.

1. Select **Add source** in the left **Sources** panel.
2. Enter the working-copy directory as **Approved directory**.
3. Enter the absolute path to `01-project-brief.md` as **Source file**.
4. Select **Archive and extract**.
5. Wait for the ingestion task to complete and the source to appear as active.
6. Repeat the process for the other five files.

Use the same approved directory each time, changing only the source filename:

```text
01-project-brief.md
02-decision-log.md
03-alert-runbook.md
04-collector-config.json
05-pilot-checklist.csv
06-sample-incident.log
```

This demonstrates that a vault can hold Markdown, JSON, CSV, and log sources.
For each file, Pinky retains its original bytes, extracted text, searchable
chunks, metadata, and version relationship.

If Pinky reports `source path is outside the approved root`, compare the two
resolved paths with `realpath`. The source path must begin with the full
approved-directory path followed by `/`. See the
[contextual ingestion errors](VAULT_GUIDE.md#if-adding-a-source-reports-an-error)
for more examples.

## Lesson 2: Find one known fact

Enter this distinctive site name in the centre search field:

```text
Northglass
```

You should see passages from more than one source, including the project brief,
configuration, checklist, or incident log. Open the result from
`01-project-brief.md`.

In the citation viewer, identify:

- the retained filename and MIME type;
- the source coordinates;
- the retrieval time;
- the original working-copy path; and
- the stable `pinky://` citation URI.

This is the core verification habit: search finds candidate evidence; opening
the citation lets you inspect the exact retained passage and provenance.

## Lesson 3: Search across related sources

Search for the fictional alert code:

```text
IRIS417
```

The decision log defines when the alert should be raised, the runbook explains
what an operator should do, the JSON config represents the machine setting,
and the sample log records an occurrence. Open at least two citations and
compare their roles.

This pattern is useful for real material: use a shared identifier such as a
ticket number, error code, product name, or policy label to connect evidence
distributed across notes, configuration, runbooks, and logs.

## Lesson 4: Ask the search engine precise questions

The current input is lexical search, so phrase your request as terms likely to
appear in the source rather than as a natural-language question.

Try these searches:

| Search | Expected evidence |
| --- | --- |
| `telemetry interval` | The 30-second decision and JSON configuration. |
| `retry_limit` | The collector delivery configuration. |
| `security review` | The pending checklist item owned by Mira Chen. |
| `dashboard notifications` | The accepted notification-channel decision. |
| `confirmed_leak` | The permitted runbook label and the sample incident resolution. |
| `sunstone` | Records in several formats that share the verification word. |

If a long search produces no useful result, reduce it to the rarest two or
three terms. Exact identifiers are usually more effective than words such as
“what,” “why,” or “summarize.” Pinky does not compose an answer yet; the result
passages are the output.

## Lesson 5: Verify, do not merely match

Search for:

```text
12 litres
```

Open the decision-log and runbook citations. Both mention the same threshold,
but they serve different purposes: one records an accepted decision and the
other gives an operational procedure. Then search `sustained_minutes` and open
the JSON citation to inspect the configured value.

Matching text is not automatically sufficient evidence. In your own vault,
check whether a source is authoritative for the question, whether it is the
current retained version, and whether another independent source disagrees.
The present build exposes evidence but does not make that judgement for you.

## Lesson 6: Create and find a new version

Open the working copy of `02-decision-log.md` in your normal text editor and add
this section at the end:

```markdown
## ALD-004: Training marker

Status: accepted for tutorial use.

The operator training marker is CANOPY88.
```

Save the file and leave Pinky running. The watcher waits for the file to become
stable, then starts a local-ingestion task. After that task completes, search:

```text
CANOPY88
```

Open the citation and confirm that it points to the updated retained version.
Earlier citation URIs still identify their original archived version even
after the source's current-version pointer advances.

If no watcher task appears after several seconds, confirm that you edited the
same canonical path you originally ingested and that the runtime panel says
the file watcher is `watching`.

New files in the same approved directory are handled in the same way: leave
them in place until their size and modification time are stable, then wait for
the **local ingestion** discovery task. The file appears in **Sources** after
that task completes.

## Lesson 7: Apply the workflow to your own material

Start with a small, purposeful collection rather than approving your entire
home directory. Good first collections include:

- project decisions plus the specification they affect;
- an operational runbook plus representative, non-secret logs;
- meeting notes grouped around one product or subject;
- versioned policies and their implementation checklists; or
- source code alongside architecture and troubleshooting notes.

For best results:

1. Use descriptive filenames and headings.
2. Keep dates, owners, identifiers, and status words explicit in the text.
3. Use consistent identifiers across related files.
4. Approve the narrowest directory that contains the selected source.
5. Confirm every ingestion task completes before relying on search.
6. Search for distinctive terms, then open citations to verify context.
7. Keep your originals and normal backups; the current vault is not a supported
   backup replacement.
8. Avoid placing credentials or unnecessary secrets in tutorial or test data.

## What this tutorial does not demonstrate

The current build now archives PNG, JPEG, WebP, GIF, and TIFF files, makes
their technical metadata searchable, can attach bounded local OCR text, and
can open retained pixels from image citations. The Asset Library can remove an
individual OCR detection or all OCR detections while retaining the image. It
does not yet summarize the pack, ingest webpages, extract Office documents,
create dossiers, or run a local language model. PDF extraction is available
when Poppler is installed; image-only pages are also OCRed when `pdftoppm` and
Tesseract are available. Office extraction remains a later milestone.
Completing this tutorial
confirms that encrypted local retention, version watching, lexical retrieval,
and exact citation reopening are working.

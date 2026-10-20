# M7: Outside sources

Status: not started. Depends on: M2. Design: [library.md](library.md#outside-sources).

**Goal.** Ask can draw on trusted public-domain commentaries, reference works and confessions, and says
whose reading it's giving.

## Sources

Each source is pinned by URL and SHA-256 in `xtask`, like the translations, and recorded in
`LICENSES/README.md` with its provenance and why it's public domain.

| Work | Kind | Likely source |
|---|---|---|
| Matthew Henry, *Commentary on the Whole Bible* | commentary | CCEL (ThML) or the SWORD `MHC` module |
| Jamieson-Fausset-Brown | commentary | SWORD `JFB` |
| Barnes' Notes | commentary | SWORD `Barnes` |
| Keil & Delitzsch, Old Testament | commentary | SWORD `KD` |
| Calvin's commentaries (Calvin Translation Society) | commentary | CCEL (ThML) |
| Easton's Bible Dictionary | dictionary | SWORD `Easton` |
| Nave's Topical Bible | topical | SWORD `Nave` |
| Apostles', Nicene and Athanasian Creeds; Westminster Confession, Larger and Shorter Catechisms; Heidelberg Catechism; Belgic Confession; Canons of Dort; 1689 London Baptist Confession | confession | public-domain editions with their proof texts |

Before building each one, confirm the specific edition is public domain (the work's translation and
editing, not just its author). The SWORD importer that reads translations (`import/sword`) already
handles zText, RawText and the markup these modules use; it gains the commentary and lexicon module
drivers.

## Building and loading

1. **`cargo xtask build-sources`** writes `data/sources/<work>.jsonl.zst` (not compiled into the apps):
   one entry per passage (commentaries: verse id ranges; confessions: sections with their proof-text
   ranges) or per term (dictionary and topical: the term, its text and the verses it cites).
2. **Shared Convex tables**, readable by every signed-in user and written only by the loader:
   - `sourceWorks`: `key`, `title`, `author`, `year`, `tradition`, `kind`, `licence`.
   - `sourceEntries`: `work`, `key` (stable: the work's own passage or term label), `from`, `to` (verse
     ids), `term?`, `heading?`, `text`, with a search index on
     `text` and an index on `term`.
   - `sourceAnchors`: one row per chapter an entry's range touches (`work`, `book`, `chapter`, `from`,
     `to`, `entryId`), indexed `[book, chapter]`, exactly as the library's anchors are, so a passage query
     finds entries whose ranges start in an earlier chapter.
3. **`server/scripts/load-sources.ts`** loads a work in batches of 500 entries through a server-secret
   mutation, replacing the previous load of that work atomically by version.

## Ask

Contract `ask/6` ([backend.md](backend.md#the-ask-contract)) adds two tools, both calling Convex:
- **`commentaries { passage, works? }`**: what each commentary and confession says on the passage, with
  the work's name, date and tradition, trimmed to about 1,500 words in total.
- **`topics { term }`**: Easton's and Nave's entries for a term, and confession sections that match it,
  each with the verses it cites.

The prompt asks the model to attribute ("Matthew Henry reads this as…"), to give more than one tradition
where they differ, and never to present a commentary as the meaning of the text. Citations of a work
render as `{{source <work> <key>}}` chips that open the entry, where `key` is the entry's stable key (the
work and its passage or term), which survives reloading the work; Convex ids are never cited.

## The apps

A "Commentary" section in the desktop's inspector panel and on iOS's verse menu shows the entries for the
selected verse, so readers can read the sources themselves, not only through Ask.

## Tests and done when

- `build-sources` reproduces byte-identical output from the pinned inputs.
- A passage query returns entries whose ranges cover it, including ranges starting in an earlier chapter.
- The eval gains 15 questions where a good answer attributes a commentary; at least 13 attribute
  correctly and none presents a commentary as the text's meaning.
- **Done when** all the works above are loaded, Ask uses them with attribution, and the licence notes are
  complete.

# M7: Outside sources

Status: the data half is built (October 21): the Sources table, `build-sources`, the SWORD drivers, the
tables, the loader and the passage query. Ask (`ask/6`, the tools, the prompt, the chips) and the apps'
Commentary section wait on M2's `ask/5`. Nothing is loaded into a deployment yet. Depends on: M2.
Design: [library.md](library.md#outside-sources).

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

**Built (October 21), where it is smaller than the above:** nine works are in (`mhc`, `jfb`, `barnes`,
`kd`, `calvin`, `easton`, `nave`, `westminster`, `lbcf`), every one a CrossWire SWORD module whose conf
says public domain, pinned in `xtask/src/sources.rs` and explained in `LICENSES/README.md`. Barnes is
the New Testament only and Calvin 48 books (no Joshua, and none he didn't write on), as the modules
are. The Apostles', Nicene and Athanasian Creeds, the Heidelberg Catechism, the Belgic Confession and
the Canons of Dort are **open**: no public-domain module holds them, and Schaff's *Creeds of
Christendom* vol. 3 on CCEL (details in `LICENSES/README.md`) has their texts in two-column tables with
page breaks inside answers, which is a cutting job of its own; they are left out of the first load.
The SWORD importer gained `read_commentary` (zCom/zCom4/RawCom, linked slots as one entry on their
range, a chapter's or book's heading slot as an entry on the whole chapter or book), `read_lexicon`
(zLD) and `read_genbook` (RawGenBook, for the confessions), and a `plain_text` that keeps a note's
references and drops its text. A confession is cut into its paragraphs (`WCF.1.1`, `LBCF.32.3`) and
questions (`WSC.1`, `WLC.196`) with the proof texts as `cites`; a commentary entry's key is its range
(`Gen.1.1-2.3`); a dictionary's its term. An entry with no text is dropped, and a key the module
repeats gets `~2`, `~3`… in module order (Easton `KADESH`, Nave `SIN`, Matthew Henry's chapter
heading next to a run of verses). `cites` are on `sourceEntries` too, so the topics tool can show them.
`sourceAnchors` come from a commentary entry's range and from a confession section's proof texts
(a dictionary or topical entry has no anchors; its door is its term), so the passage query returns
confession sections on a passage as the tool needs. Loading is three server-secret functions,
`sources.load` (a batch under a new `version`), `sources.commit` (writes the work's row with that
version: one write, so the swap is atomic) and `sources.purge` (the replaced version's rows in steps
of 500), plus `sources.works` so the loader skips an unchanged file (the version is the file's
SHA-256) and knows what to purge. `sources.passage { from, to, works? }` is a user query, as the
library's tools are called by the apps; a signed-in user with no `users` row yet gets `[]`, like
`highlights.list`. The term lookup for the topics tool is not written; the index is. No deployment
has been loaded. Easton's module tags some continuation references ("Rev. 1:8, 11") with the wrong
book; the citations are kept as the module has them and noted in `LICENSES/README.md`. The
byte-identity test (`checked_in_files_match_the_pinned_inputs`) only checks the works whose pinned
zip is in `.sources/`, so it runs where `build-sources` has been run and never in CI, where
`.sources/` is empty.

## Ask

Contract `ask/6` ([backend.md](backend.md#the-ask-contract)) adds two tools, both through
`LibraryBackend` like the library's own:
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

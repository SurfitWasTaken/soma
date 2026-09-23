# PRD — Soma

**A local-first, native Rust PDF reader whose annotations are a typed graph.**

| | |
|---|---|
| Status | Draft v0.1 |
| Date | 2026-09-23 |
| Codename | Soma *(placeholder — anatomical metaphor: organs belong to multiple systems)* |
| Platform | macOS first, Linux second, Windows third |
| Language | Rust, native GPU rendering, no browser runtime |

---

## 1. Problem

Reading dense technical material produces two kinds of artifacts, and current tools only handle the first:

1. **Marks on the page** — highlights, margin notes. Every PDF reader does this. The notes are trapped where you made them, ordered by page, which is the one ordering that has nothing to do with how you think.
2. **The structure of your confusion** — *"I don't understand term X"*, *"X is a prerequisite for Y"*, *"these two definitions look like they contradict each other"*, *"I follow A and I follow B but not the step between them."* This is a graph, and it is the artifact that actually determines whether you finish the paper.

Existing tools force a bad trade. PDF readers (Preview, Zotero, Skim) capture (1) and discard (2). Graph tools (Obsidian, Roam, Heptabase, Logseq) capture a flattened version of (2) but treat the PDF as a second-class citizen behind an export step, and they model relationships as untyped, contentless lines. Nothing supports the move that matters most: **treating a relationship as a thing you can be confused about.**

### 1.1 The specific gap

In every graph tool, an edge is a line. If your actual thought is *"I don't understand how the martingale property relates to the no-arbitrage condition"*, you have three bad options:

- **Write it in one of the nodes.** Now the note is asymmetric and lives under the wrong concept. The other endpoint doesn't know about it.
- **Insert a node between them** (`A → "unclear link" → B`). This is the common workaround and it is wrong on three counts: it destroys the direct `A—B` relation (you now have two relations where you meant one), it promotes a piece of *commentary about a link* into a *concept*, and it corrupts every downstream query — path length between A and B is now 2, prerequisite chains inherit a fake step, and layout algorithms route around a thing that isn't there.
- **Make a floating note** with no graph position at all, which is where notes go to die.

The correct answer is **reification**: a relation has its own identity, can carry content, can belong to a system, and can itself be the endpoint of another relation — *without changing the topology of the graph it sits in.* `A—B` stays exactly one relation whether or not it is annotated. This is the central design commitment of this product and Section 4.4 specifies it.

---

## 2. Goals and non-goals

### 2.1 Goals

| # | Goal | Measured by |
|---|---|---|
| G1 | Reading feels like a native PDF reader, not an annotation tool with a PDF in it | Page render p95 < 80 ms; scroll at display refresh rate; cold open of a 400-page PDF < 1.2 s |
| G2 | Capture is faster than the thought | Selection → highlighted, filed node: **2 keystrokes**. Selection → node linked to previous node: **3 keystrokes**. No mouse required, no dialog. |
| G3 | Relationships are first-class and typed | Every relation has a kind; any relation can carry content, join a system, and be an endpoint — with zero topology change |
| G4 | Multiple overlapping classification schemes over one corpus | An entity can belong to *n* systems; overlays compose (union / intersection / difference) |
| G5 | The graph and the document are two views of one dataset | Any node ↔ its source region, bidirectional, one keystroke, < 100 ms |
| G6 | Everything is local and inspectable | Single SQLite file per workspace; full JSON + Markdown export; no account, no network calls |

### 2.2 Non-goals (v1)

- **No cloud sync, no collaboration, no sharing.** Single user, single machine. (Workspace file is sync-safe on Dropbox/iCloud in a single-writer sense; concurrent multi-device editing is out of scope.)
- **No PDF editing.** We never write back into the source PDF. Annotations live in our own store. (Optional *export* to standard PDF annotations is a stretch goal, explicitly lossy — it cannot represent typed relations.)
- **No AI features in v1.** No auto-summarization, no embedding search, no suggested links. Section 12 sketches where they'd go in v2; v1 must be good without them.
- **No web version, no mobile, no Electron.**
- **No general-purpose note-taking.** Notes exist because a document produced them. Orphan nodes are supported (Section 5.3) but are not the primary flow.
- **No OCR in v1.** Scanned PDFs without a text layer get region-anchored notes only (no text anchoring). OCR is v2.
- **No EPUB/DjVu/HTML in v1.** The anchoring model is designed to extend to them (Section 4.2) but only PDF ships.

---

## 3. User and primary scenario

**Primary user:** a technical reader working through material at the edge of their competence — graduate coursework, research papers, specifications, textbooks. Comfortable with modal editors and keyboard-driven tools. Reads the same document many times over weeks. Values the ability to reconstruct *why past-them was confused*.

**Primary scenario — the prerequisite chain.**

> Reading a stochastic calculus text. Page 84 uses "quadratic variation" in a proof. I don't follow it.
>
> `Ctrl-1` — the phrase under the cursor is highlighted in the "lapse in understanding" color and a node is created, titled from the selection, filed in that system. I keep reading.
>
> Page 91 uses "Itô isometry", which I also don't follow — and I can tell it's downstream of quadratic variation.
>
> `Ctrl-1`, then `L` — node created, and linked `prerequisite-of` from the previous node I made. Three keystrokes. Never left the page.
>
> Later I open the graph, turn on only the "lapse in understanding" overlay, and see a 7-node dependency chain with two roots. The roots are what I actually need to go read about. One of the relations — between "quadratic variation" and "Itô isometry" — has a chip on it: past-me wrote *"I see the algebra but not why the limit exists."* That relation is itself in the system, because the gap **is** the relation, not either endpoint.

**Secondary scenario — switching cost between abstractions.**

> The same paper describes a system at three levels: measure-theoretic, algorithmic, and implementation. I tag nodes with abstraction levels and set `friction` on the relations that cross levels. The graph renders cross-level relations by weight, so I can see exactly where the translation between "the math" and "the code" is expensive — which is where I will keep getting lost, and where a written bridge note pays for itself.

---

## 4. Domain model

This section is normative. Everything in Section 5 is an interface onto this model.

### 4.1 Vocabulary

| Term | Definition |
|---|---|
| **Workspace** | A directory + SQLite file holding documents, entities, systems, and views. The unit of backup and export. |
| **Document** | A PDF, identified by content hash. Stored by reference (path) or copied into the workspace. |
| **Anchor** | A durable pointer from an entity into a region of a document. |
| **Entity** | Anything addressable in the graph. Exactly one of: **Node** or **Relation**. |
| **Node** | A unit of thought. Has title, body, zero or more anchors, zero or more system memberships. |
| **Relation** | A typed, directed connection between entities. *Also an entity.* May carry title/body/anchors/memberships. |
| **Relation kind** | A named, directed edge type (`prerequisite-of`, `contradicts`, …) with display rules and constraints. |
| **System** | A named collection of entities — a classification scheme. Entities belong to many systems. |
| **Overlay** | A visibility/styling rule over systems, applied to the graph canvas. |
| **View** | A saved canvas state: overlay set, layout, filters, camera, pins. |

### 4.2 Anchors

An anchor must survive the document being re-downloaded, re-exported, or minorly revised. Store redundant locators and degrade gracefully.

```
Anchor {
  entity_id
  document_id          // content hash of the source at capture time
  page_index
  kind                 // TextRange | Region | Object
  quads: [Quad]        // page-space quadrilaterals, for instant redraw
  exact:  String       // the selected text
  prefix: String       // 48 chars before
  suffix: String       // 48 chars after
  char_start, char_end // offsets into the page's extracted text
  confidence: f32      // 1.0 at capture; recomputed on re-anchor
}
```

**Resolution order** when opening a document whose hash differs from capture:

1. `char_start..char_end` on the same page → verify `exact` matches. Exact hit, confidence 1.0.
2. Fuzzy search for `prefix + exact + suffix` on the same page, then ±2 pages, then whole document. Confidence from edit distance.
3. Fall back to `quads` geometry on the same page index. Confidence 0.3, flagged.
4. Unresolved → node is marked **detached**. It stays in the graph, keeps all relations, and shows a "re-anchor" affordance. **Losing an anchor must never lose a node.**

`Region` anchors (rectangle drag over a figure/equation) and `Object` anchors (a clicked image or vector group) skip steps 1–2 and use geometry only.

Anchoring is deliberately document-format-agnostic below the `kind` field, so EPUB/HTML can be added later with a CFI/XPath locator variant.

### 4.3 Nodes

```
Node {
  entity_id
  title: String             // auto-filled from selection, editable
  body: String              // markdown, optional
  abstraction_level: Option<i8>   // -3..=3, user-defined ladder; drives friction analysis
  color_override: Option<Color>
  created_at, updated_at
}
```

A node may have **zero** anchors (a synthesized concept), **one** (the common case), or **many** (the same idea appearing in five places across three papers — one node, five anchors). Anchors carry no semantics beyond "this entity is evidenced here."

### 4.4 Relations — the core mechanism

```
Relation {
  entity_id
  kind_id                  // prerequisite-of, contradicts, …
  endpoints: [Endpoint]    // ordered; v1 UI always creates exactly 2
  friction: Option<f32>    // 0.0..=1.0 — switching cost across this link
  title: Option<String>    // Some(_) ⇒ the relation is annotated
  body: Option<String>
}

Endpoint {
  relation_id
  entity_id                // ← an entity, therefore possibly another Relation
  role                     // Source | Target | Member
  ordinal
}
```

Two decisions make this work:

**(a) Endpoints reference `entity`, not `node`.** A relation can point at a relation. This is what makes *"I don't understand the link between A and B"* expressible without inventing a fake concept.

**(b) Endpoints are a table, not two columns.** Binary relations are the 2-row case. This costs one join in v1 and buys n-ary relations (*"I don't see how A, B and C fit together"*) with no migration. The v1 UI only creates binary relations; the schema does not need to change to lift that restriction.

#### Three states of a relation

A relation is always exactly one relation. Its *state* is derived from its content and memberships — there is no separate "convert to node" operation that changes topology.

| State | Condition | Rendering |
|---|---|---|
| **Bare** | no title/body, no memberships | A line. Kind shown by stroke style/color. |
| **Annotated** | has title or body | A line with a small chip at the midpoint. Hover/focus shows the body. |
| **Promoted** | has content **and** belongs to ≥1 system, or is an endpoint of another relation | A lozenge rendered *inline on the edge*: `(A)——◇——(B)`. Other relations attach to the lozenge. |

```
   Plain graph tool's only option (WRONG):        Soma (CORRECT):

        (A)                                            (A)
         |  prerequisite-of                             |
         v                                              |  prerequisite-of
   [ unclear link ]   ← a fake concept                  ◇  "I see the algebra but
         |              now in your concept space       |   not why the limit exists"
         v                                              |   ∈ lapse-in-understanding
        (B)                                             v
                                                       (B)
   2 relations. Path A→B = 2.              1 relation. Path A→B = 1.
   Deleting the commentary               The commentary is *on* the edge;
   breaks the A—B link.                  deleting it leaves A—B intact.
```

**Invariants (must hold, enforced in the data layer):**

- `I1` Promoting or demoting a relation never creates, deletes, or redirects any relation. Topology is invariant under annotation.
- `I2` Deleting an entity cascades to every relation with it as an endpoint, recursively. Deleting `A` removes `A—B` and anything attached to `A—B`. The UI shows the full cascade count before confirming.
- `I3` A relation may not be, transitively, its own endpoint. Attempts are rejected at write time.
- `I4` Relation-on-relation nesting depth is capped at 4. Beyond that, the UI suggests the thought has become a concept and offers to create a node. (This is the one place where the "make it a node" escape hatch is correct — offered, never forced.)
- `I5` Every entity in a system renders when that system's overlay is on, including relations whose endpoints are *not* in the system (see 5.5, ghost endpoints).

#### Relation kinds

Shipped defaults, each with directedness, default stroke, and constraints:

| Kind | Directed | Semantics | Constraint |
|---|---|---|---|
| `prerequisite-of` | → | must understand source before target | acyclic; cycles flagged, not blocked |
| `elaborates` | → | target expands the source | — |
| `contradicts` | ↔ | the two cannot both be right as stated | — |
| `same-as` | ↔ | different names for one thing | transitive; offers merge |
| `instance-of` | → | target is a general case of source | acyclic |
| `depends-on` | → | generic dependency | — |
| `unclear-link` | ↔ | *I don't know how these relate* | auto-joins the active "confusion" system |

Kinds are user-editable and per-workspace. A system may declare a preferred kind vocabulary, so the link palette shows the right 4 options first rather than all 20.

### 4.5 Systems and overlays

The anatomy metaphor is load-bearing: the femur is in the skeletal system; it is also in the locomotor system; it is not *copied* into either. Systems are **many-to-many over entities, including relations.**

```
System {
  id, name, description
  color                    // drives node tint and highlight color in the reader
  default_relation_kinds: [kind_id]
  default_layout           // Force | Layered | Radial | Manual
  hotkey: Option<u8>       // Ctrl-1..Ctrl-9 in the reader
}
```

Examples a user would actually create: *lapse in understanding*, *terminology*, *proof obligations*, *open questions*, *things to verify empirically*, *contradicts my prior*, *worth stealing*.

**Overlay composition.** The canvas holds a set of active systems and a combining mode:

- **Union** (default) — show entities in any active system.
- **Intersection** — show only entities in *all* active systems. (*"what is both a terminology gap and a proof obligation"*)
- **Difference** — A minus B. (*"confusions I haven't yet turned into open questions"*)
- **Ghost mode** (independent toggle) — out-of-overlay entities render at 12% opacity instead of being hidden, so removing an overlay doesn't visually shatter the structure. On by default; `G` toggles.

### 4.6 Switching cost between abstractions

`Node.abstraction_level` places a node on a user-defined ladder (e.g. `-1` implementation, `0` algorithmic, `+1` formal). `Relation.friction` is a 0–1 scalar for *how expensive it is to move across this link*.

Derived affordances:

- Relations are auto-flagged as **cross-level** when `|Δlevel| ≥ 1`; these render with a distinct dashed-through-gradient stroke.
- Layout treats `friction` as edge length — high-friction links are drawn long. Expensive translations are literally far apart on screen.
- A **Friction lens** (Section 5.6) recolors the whole graph by friction and lists the top-N highest-cost crossings. These are the bridge notes worth writing.
- Friction is set with `Ctrl-Shift-1..5` on a focused relation, or inferred (default `0.5`) for any cross-level relation until set.

---

## 5. Functional requirements

Priorities: **P0** = v1 cannot ship without it. **P1** = v1 should have it. **P2** = fast-follow.

### 5.1 Reader (F-READ)

| ID | Requirement | Pri |
|---|---|---|
| F-READ-1 | Open local PDFs; continuous vertical scroll; two-page spread; fit-width / fit-page / manual zoom 25–800% | P0 |
| F-READ-2 | GPU-composited rendering; tile cache; render current page ±2 ahead, evict by LRU | P0 |
| F-READ-3 | Text selection: click-drag, double-click word, triple-click line/paragraph, shift-extend | P0 |
| F-READ-4 | Rectangular region selection for figures, tables, equations (`Ctrl-drag`) | P0 |
| F-READ-5 | **Object hit-testing on click**: a single click identifies the token, image, or vector group under the cursor and makes it the implicit selection — so capture works with no drag at all | P0 |
| F-READ-6 | Outline/bookmarks pane; internal link navigation; back/forward stack | P0 |
| F-READ-7 | Full-text search in document with match list, `n`/`N` to cycle | P0 |
| F-READ-8 | Tabs for multiple open documents within a workspace | P1 |
| F-READ-9 | Dark mode / sepia / inverted render, without wrecking figure colors | P1 |
| F-READ-10 | Annotation gutter: a thin margin strip showing where marks exist on the current page and in the scrollbar for the whole document | P1 |
| F-READ-11 | Render existing PDF annotations from the file read-only, visually distinguished from our own | P2 |

### 5.2 Capture (F-CAP)

The requirement behind all of these is **zero-latency, zero-dialog capture**. Any flow that opens a modal before the user can keep reading has failed.

| ID | Requirement | Pri |
|---|---|---|
| F-CAP-1 | `h` — highlight current selection with the last-used color. No node created. Paint appears in the same frame. | P0 |
| F-CAP-2 | `Ctrl-<n>` — highlight in system *n*'s color **and** create a node in system *n*, titled from the selection, anchored, with the surrounding sentence captured as body. No dialog. A 1.5 s toast shows the title with an undo affordance. | P0 |
| F-CAP-3 | `N` — same as above but opens an inline composer at the selection for title/body, `Esc` cancels, `Ctrl-Enter` commits | P0 |
| F-CAP-4 | `L` — link the just-created node to the **previously created node**, using the active system's first default relation kind. This is the prerequisite-chain shortcut. | P0 |
| F-CAP-5 | `l` — link the just-created node to an arbitrary target: a hint overlay (2-char labels, Vimium-style) appears over recently-touched nodes and over the inline graph strip; typing the label completes the link. Falls through to fuzzy search on `/`. | P0 |
| F-CAP-6 | Relation kind picker: after `L`/`l`, a single keypress from a 4-item palette overrides the default kind. Ignoring it commits the default. | P0 |
| F-CAP-7 | Multi-anchor: with a node focused, `a` adds the current selection as an additional anchor to that node | P1 |
| F-CAP-8 | Highlight colors: 9 slots, bound to systems by default, overridable | P1 |
| F-CAP-9 | Strikethrough, underline, freehand ink | P2 |
| F-CAP-10 | **Term recognition**: when a selection's text exactly matches an existing node's title or a registered alias in this workspace, offer (single keypress) to anchor to the *existing* node instead of creating a duplicate | P1 |

### 5.3 Nodes (F-NODE)

| ID | Requirement | Pri |
|---|---|---|
| F-NODE-1 | Create from selection, from region, or free-standing (`Ctrl-N` on the canvas) | P0 |
| F-NODE-2 | Edit title/body in markdown; body supports links to other entities via `[[` autocomplete | P0 |
| F-NODE-3 | Jump node → source: focus a node, press `Enter`, reader opens the document at the anchor and flashes the region | P0 |
| F-NODE-4 | Jump source → node: click a highlight in the reader, its node(s) focus in the graph strip; `Ctrl-Enter` opens full canvas focused there | P0 |
| F-NODE-5 | Set/clear system membership on a focused node: `s` opens the system palette, multi-select | P0 |
| F-NODE-6 | Set abstraction level: `Ctrl-Shift-Up/Down` | P1 |
| F-NODE-7 | Merge two nodes (union of anchors, relations, memberships; one title survives; undoable) | P1 |
| F-NODE-8 | Detached-node repair UI: list of nodes whose anchors failed to resolve, with a "find in document" re-anchor flow | P1 |

### 5.4 Relations (F-REL)

| ID | Requirement | Pri |
|---|---|---|
| F-REL-1 | Create by keyboard (`l`, hint-select) or by cursor (drag from a node's rim to a target; release on empty space opens "create node and link") | P0 |
| F-REL-2 | Create between two selected nodes: rubber-band or `Ctrl-click` two nodes, press `l` | P0 |
| F-REL-3 | **Annotate a relation**: focus a relation (`e` cycles edges of the focused node, or click the line) and press `N`. Title/body composer opens. The relation gains a midpoint chip. **Topology unchanged.** | P0 |
| F-REL-4 | **File a relation into a system**: focus a relation, press `s`, choose systems. The relation now renders as a promoted lozenge and appears in that system's overlay and lists, exactly like a node does. | P0 |
| F-REL-5 | **Relate to a relation**: with a relation focused, `l` starts a link *from that relation*; hint targets include other relations' midpoint handles | P0 |
| F-REL-6 | Change relation kind (`k`), reverse direction (`Ctrl-r`), delete (`Ctrl-Backspace`, with cascade preview) | P0 |
| F-REL-7 | Set friction: `Ctrl-Shift-1..5` maps to 0.1/0.3/0.5/0.7/0.9 | P1 |
| F-REL-8 | Relations may carry anchors — the passage where the juxtaposition occurs (`a` with a selection active) | P1 |
| F-REL-9 | Cycle detection on acyclic kinds: non-blocking warning badge + a "show cycle" action | P1 |
| F-REL-10 | n-ary relations via UI (schema already supports it) | P2 |

### 5.5 Graph canvas (F-GRAPH)

| ID | Requirement | Pri |
|---|---|---|
| F-GRAPH-1 | GPU-rendered pan/zoom canvas; 60 fps with 10,000 visible entities | P0 |
| F-GRAPH-2 | Layouts: force-directed (incremental, friction-weighted), layered/Sugiyama (for `prerequisite-of` DAGs), radial from a focus node, manual with pinning | P0 |
| F-GRAPH-3 | Promoted relations render as inline lozenges on their edge; relation-on-relation edges terminate on the lozenge | P0 |
| F-GRAPH-4 | **Ghost endpoints**: when an overlay shows a relation whose endpoints are outside the overlay, endpoints render as small labeled stubs rather than vanishing — the relation is never orphaned | P0 |
| F-GRAPH-5 | Focus mode: `f` on a node shows only its *k*-hop neighborhood; `[`/`]` adjust *k* | P0 |
| F-GRAPH-6 | Inline graph strip: a collapsible panel docked beside the reader showing the local neighborhood of whatever was last touched, so linking never requires a context switch | P0 |
| F-GRAPH-7 | Search/filter: by title, body, system, kind, abstraction level, document, date, anchor state | P0 |
| F-GRAPH-8 | Saved views (overlay set + layout + camera + pins), switchable by hotkey | P1 |
| F-GRAPH-9 | Roots/leaves report for a DAG kind: *"these 3 nodes have no prerequisites — start here"* | P1 |
| F-GRAPH-10 | Minimap for large graphs | P2 |
| F-GRAPH-11 | Timeline scrubber: replay the graph as it was built | P2 |

### 5.6 Overlays and lenses (F-OVL)

| ID | Requirement | Pri |
|---|---|---|
| F-OVL-1 | Toggle each system's overlay independently; `Ctrl-Shift-<n>` toggles system *n* | P0 |
| F-OVL-2 | Combining modes: union / intersection / difference | P0 |
| F-OVL-3 | Ghost mode toggle (`G`) — dim rather than hide | P0 |
| F-OVL-4 | Isolate (`i`) — solo the system under the cursor, `Esc` restores | P1 |
| F-OVL-5 | **Friction lens** — recolor by `friction`, ranked list of highest-cost crossings, cross-level relations emphasized | P1 |
| F-OVL-6 | **Abstraction lens** — lay out on a vertical axis by `abstraction_level`, making level-crossings visually explicit | P1 |
| F-OVL-7 | **Recency lens** — color by `updated_at` | P2 |
| F-OVL-8 | **Density lens** — heat by document position; shows which pages generated the most confusion | P2 |

### 5.7 Persistence and interchange (F-DATA)

| ID | Requirement | Pri |
|---|---|---|
| F-DATA-1 | One SQLite file per workspace; WAL mode; every mutation in a transaction | P0 |
| F-DATA-2 | Documents referenced by absolute path + content hash; "copy into workspace" option; broken-path relink flow | P0 |
| F-DATA-3 | Undo/redo across all mutations, ≥200 steps, persisted across restart | P0 |
| F-DATA-4 | Crash safety: no data loss on `SIGKILL` beyond the in-flight edit | P0 |
| F-DATA-5 | Export: full JSON (lossless, versioned schema) | P0 |
| F-DATA-6 | Export: Markdown — one file per system, nodes as sections, relations as typed bullets, annotated relations as their own sections | P1 |
| F-DATA-7 | Export: GraphML / DOT for external graph tooling | P2 |
| F-DATA-8 | Import: Zotero/Mendeley highlight import, Hypothesis JSON | P2 |
| F-DATA-9 | Export: flatten highlights into a copy of the PDF as standard annotations (explicitly lossy — relations are dropped, documented in the dialog) | P2 |

---

## 6. Interaction design

### 6.1 Principles

1. **Two modes, one mental model.** *Reader* and *Canvas* share a single command vocabulary; `Tab` switches. `n` creates a node in both. `l` links in both.
2. **Capture never blocks reading.** Every P0 capture action is non-modal and completes in-place. Composers are inline, anchored to the selection, and dismissible with `Esc`.
3. **Last-target memory.** The system remembers the last created node, last used relation kind, last used system, last highlight color. This is what turns 8 keystrokes into 3.
4. **Hints over pointing.** Target selection uses 2-char labels over candidates (Vimium model), not mouse hunting. The mouse always works; it's never required.
5. **Undo is sacred.** Every destructive action is undoable, including cascades. Confirmation dialogs appear only for cascades affecting >3 entities.

### 6.2 Keymap (defaults, fully rebindable)

**Reader**

| Key | Action |
|---|---|
| `h` | Highlight selection, last color |
| `Ctrl-1`…`Ctrl-9` | Highlight + create node in system *n* |
| `N` | Create node with inline composer |
| `L` | Link new node → previous node, default kind |
| `l` | Link → hint-selected target |
| `a` | Add selection as another anchor to focused node |
| `Ctrl-drag` | Region selection |
| `/`, `n`, `N` | Document search, next, previous |
| `Tab` | Switch to canvas |
| `Ctrl-Enter` | Open focused node in full canvas |
| `g`/`G` | Top / bottom of document |

**Canvas**

| Key | Action |
|---|---|
| `hjkl` / arrows | Move focus along edges |
| `e` | Cycle through focused node's relations |
| `Enter` | Jump to source anchor in reader |
| `N` | Edit title/body (works on nodes **and** relations) |
| `l` / `L` | Link from focus |
| `s` | System membership palette |
| `k` | Change relation kind |
| `Ctrl-r` | Reverse relation |
| `Ctrl-Shift-1..5` | Set friction |
| `Ctrl-Shift-Up/Down` | Abstraction level |
| `f`, `[`, `]` | Focus mode, adjust hops |
| `Ctrl-Shift-<n>` | Toggle overlay *n* |
| `G` | Ghost mode |
| `i` | Isolate system |
| `1`…`9` | Switch saved view |
| `Ctrl-Backspace` | Delete (with cascade preview) |

### 6.3 Layout

```
┌──────────────────────────────────────────────────────────────┐
│  doc.pdf ×   notes.pdf ×                          [⊞ Canvas] │
├────────────┬─────────────────────────────────┬───────────────┤
│  Outline   │                                 │  Graph strip  │
│  Systems   │        PDF page                 │               │
│  ─────────  │                                 │    (A)        │
│ ● lapse    │   ...the quadratic variation    │     │ prereq  │
│ ● terminol │   ▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓▓ of the      │     ◇ ←chip   │
│ ○ proofs   │   process is defined as...      │     │         │
│ ○ open Q   │                                 │    (B)        │
│            │                                 │               │
│            │                              ▐  │  [focused]    │
│            │                              ▐← │  B · Itô iso  │
│            │                        gutter ▐  │  ∈ lapse     │
└────────────┴─────────────────────────────────┴───────────────┘
```

The graph strip is the key ergonomic bet: the local neighborhood is always visible while reading, so linking has a visible target and capture never requires leaving the page.

---

## 7. Technical architecture

### 7.1 Stack

| Concern | Choice | Rationale |
|---|---|---|
| PDF render + text extraction | **`pdfium-render`** (PDFium via FFI) | The only option with production-grade rendering fidelity and per-character bounding boxes, which anchoring requires. MuPDF (`mupdf-rs`) is the fallback; license (AGPL/commercial) is the reason it's second. Pure-Rust (`pdf`, `lopdf`) is not viable for rendering in v1 — reassess for v2. |
| UI | **`egui` / `eframe` on `wgpu`** | Immediate-mode fits a tool that is mostly custom-drawn surfaces (page canvas, graph canvas). Fast to iterate, single binary, no web runtime. Trade-off: weaker native text input and accessibility — budget explicit work for IME and VoiceOver. `iced` is the alternative if retained-mode widgets matter more than draw-loop control. |
| Graph storage | **SQLite via `rusqlite`** (bundled) | Single file, transactional, queryable by the user with any tool. Recursive CTEs handle cascade and reachability. |
| In-memory graph | **`petgraph`**, rebuilt from SQLite on load, mutations written through | Gives us layout/traversal/cycle-detection for free. |
| Force layout | Custom Barnes–Hut on top of `petgraph`, friction as spring rest-length | Off-the-shelf crates don't support weighted rest lengths or incremental settle. |
| Layered layout | Sugiyama implementation over the `prerequisite-of` subgraph | Prerequisite chains must read top-to-bottom, not as a hairball. |
| Text search | SQLite **FTS5** over node/relation text + extracted page text | No extra dependency; good enough at this corpus size. |
| Markdown | `pulldown-cmark` | — |
| Hashing | `blake3` | Document identity. |

### 7.2 Process and thread model

Single process. Three long-lived threads plus a pool:

- **UI thread** — event loop, draw. Never blocks. Talks to everything else over channels.
- **Render worker** — PDFium page rasterization at target zoom into a tile cache. PDFium is not thread-safe across documents; serialize per-document access behind a single worker with a request queue that drops stale requests on scroll.
- **Store worker** — all SQLite writes, serialized. Reads from the UI thread go through a read-only connection pool.
- **Layout worker** — runs force simulation between frames, publishes positions via double-buffered snapshot. Cancellable.

**Latency budget** (from keypress to visible result):

| Action | Budget |
|---|---|
| Highlight paint | < 16 ms (same frame; optimistic, DB write async) |
| Node created and visible in strip | < 50 ms |
| Page render at new zoom | < 80 ms p95 |
| Graph → reader jump | < 100 ms |
| Overlay toggle, 10k entities | < 33 ms |
| Cold open, 400-page PDF | < 1.2 s to first page |
| Workspace load, 50k entities | < 2 s |

All mutations are applied **optimistically** to the in-memory graph and rendered immediately; the store worker persists asynchronously and signals failure by reverting with a toast. This is what makes capture feel free.

### 7.3 Schema (abridged)

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE document (
  id            TEXT PRIMARY KEY,          -- blake3 of file bytes
  path          TEXT NOT NULL,
  title         TEXT,
  page_count    INTEGER NOT NULL,
  copied_local  INTEGER NOT NULL DEFAULT 0,
  added_at      INTEGER NOT NULL
);

-- Every addressable thing in the graph.
CREATE TABLE entity (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL CHECK (kind IN ('node','relation')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE node (
  entity_id         TEXT PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  title             TEXT NOT NULL,
  body              TEXT NOT NULL DEFAULT '',
  abstraction_level INTEGER,
  color_override    INTEGER
);

CREATE TABLE relation_kind (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL,
  directed   INTEGER NOT NULL DEFAULT 1,
  acyclic    INTEGER NOT NULL DEFAULT 0,
  stroke     TEXT NOT NULL
);

CREATE TABLE relation (
  entity_id TEXT PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  kind_id   TEXT NOT NULL REFERENCES relation_kind(id),
  friction  REAL,
  title     TEXT,                          -- non-null ⇒ annotated
  body      TEXT
);

-- Endpoints reference ENTITY, so a relation can point at a relation.
-- Table (not two columns) so n-ary relations need no migration.
CREATE TABLE relation_endpoint (
  relation_id TEXT NOT NULL REFERENCES relation(entity_id) ON DELETE CASCADE,
  entity_id   TEXT NOT NULL REFERENCES entity(id)          ON DELETE CASCADE,
  role        TEXT NOT NULL CHECK (role IN ('source','target','member')),
  ordinal     INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (relation_id, entity_id, role, ordinal)
);
CREATE INDEX idx_endpoint_entity ON relation_endpoint(entity_id);

CREATE TABLE system (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT,
  color INTEGER NOT NULL, hotkey INTEGER, default_layout TEXT NOT NULL DEFAULT 'force'
);

-- Membership is over ENTITY: a relation joins a system exactly like a node does.
CREATE TABLE membership (
  system_id TEXT NOT NULL REFERENCES system(id) ON DELETE CASCADE,
  entity_id TEXT NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  added_at  INTEGER NOT NULL,
  PRIMARY KEY (system_id, entity_id)
);

-- Anchors are over ENTITY: relations can cite a passage too.
CREATE TABLE anchor (
  id          TEXT PRIMARY KEY,
  entity_id   TEXT NOT NULL REFERENCES entity(id)   ON DELETE CASCADE,
  document_id TEXT NOT NULL REFERENCES document(id) ON DELETE CASCADE,
  page_index  INTEGER NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('text','region','object')),
  quads       BLOB NOT NULL,
  exact       TEXT NOT NULL DEFAULT '',
  prefix      TEXT NOT NULL DEFAULT '',
  suffix      TEXT NOT NULL DEFAULT '',
  char_start  INTEGER, char_end INTEGER,
  confidence  REAL NOT NULL DEFAULT 1.0,
  color       INTEGER
);
CREATE INDEX idx_anchor_doc_page ON anchor(document_id, page_index);

CREATE TABLE view (
  id TEXT PRIMARY KEY, name TEXT NOT NULL, hotkey INTEGER,
  state JSON NOT NULL                      -- overlays, mode, camera, pins, lens
);

CREATE TABLE undo_log (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL, forward JSON NOT NULL, inverse JSON NOT NULL
);

CREATE VIRTUAL TABLE entity_fts USING fts5(entity_id UNINDEXED, title, body);
```

Note the shape of the model in three lines: **systems contain entities; entities are nodes or relations; relations connect entities.** Everything in Section 4.4 falls out of that, including the thing the product exists for.

### 7.4 Crate layout

```
soma/
  crates/
    soma-core/      domain types, invariants, graph ops — no I/O
    soma-store/     SQLite, migrations, undo log, FTS
    soma-pdf/       pdfium wrapper, tile cache, text extraction, anchor resolution
    soma-layout/    force, sugiyama, radial
    soma-ui/        egui app, reader, canvas, palettes, keymap
    soma-cli/       headless export/import/verify (also the test harness)
  app/              platform bundle, icons, packaging
```

`soma-core` is pure and exhaustively unit-tested — the invariants in 4.4 are property tests there, not UI assertions.

---

## 8. Success criteria

### 8.1 Acceptance tests (must pass to ship v1)

| # | Test |
|---|---|
| A1 | From a fresh selection, a filed, highlighted, anchored node exists after exactly 2 keystrokes |
| A2 | Two nodes created on different pages are linked `prerequisite-of` in 3 keystrokes from the second selection |
| A3 | A relation can be given a title, filed into a system, and made the target of a second relation — and the original endpoints' shortest path remains 1 |
| A4 | Deleting an endpoint node removes the relation and everything attached to it, and one undo restores all of it |
| A5 | Re-downloading a paper with different byte content re-anchors ≥95% of text anchors at confidence ≥0.9; no node is lost |
| A6 | 10,000 entities, 20,000 relations: overlay toggle < 33 ms, sustained 60 fps pan/zoom |
| A7 | `SIGKILL` during heavy annotation loses at most the in-flight edit; workspace opens clean |
| A8 | Full JSON export round-trips to a byte-identical graph |

### 8.2 Product signals (post-launch)

- Median relations per node > 1.2 — if users only make nodes, the graph thesis is wrong.
- ≥15% of relations are annotated or promoted — if nobody reifies, Section 4.4 was an over-build and we simplify.
- Documents opened ≥3 times across ≥3 days — the tool is for re-reading, not skimming.
- Time-to-first-node on a fresh document < 90 s.

---

## 9. Milestones

| M | Scope | Exit criterion |
|---|---|---|
| **M0** Spike (2 wk) | pdfium in egui: render, scroll, select text, get quads + char offsets | Text selection with correct quads on 5 diverse real PDFs, including a two-column paper |
| **M1** Reader (3 wk) | Continuous scroll, zoom, outline, search, tabs, dark mode, region select, object hit-test | F-READ P0 complete; latency budget met |
| **M2** Capture (3 wk) | Anchors + persistence, highlights, node creation, systems, `Ctrl-<n>` flow, undo | A1 passes; kill-test A7 passes |
| **M3** Canvas (4 wk) | Graph rendering, force + layered layout, focus mode, graph strip, bidirectional jump | A2, A6 pass |
| **M4** Reification (3 wk) | Relation annotation, promotion, relation-on-relation, cascade + invariants, inline lozenge rendering | A3, A4 pass; property tests green |
| **M5** Overlays (3 wk) | Composition modes, ghost endpoints, saved views, friction + abstraction lenses | F-OVL P0/P1 complete |
| **M6** Durability (3 wk) | Re-anchoring, detached repair, exports, packaging, crash recovery | A5, A8 pass; signed macOS build |

≈21 weeks to a v1 worth using daily. M0–M4 is the minimum coherent product; M5 is what makes it *this* product rather than a nicer PDF reader.

---

## 10. Risks

| Risk | Impact | Mitigation |
|---|---|---|
| PDFium FFI instability / licensing / build complexity across platforms | High | Isolate behind a `soma-pdf` trait boundary in M0 so MuPDF or a pure-Rust backend can be swapped. Vendor prebuilt PDFium binaries per target. |
| Text anchoring fails on two-column, ligature-heavy, or badly-encoded PDFs | High | M0 tests against an adversarial corpus (two-column ACM, LaTeX with ligatures, scanned+OCR'd, CJK). Region anchors always work as fallback. |
| egui text input / IME / accessibility gaps | Medium | Budget explicit IME work in M2. If it blocks, note composers fall back to a platform-native text field overlay. |
| **Reification is over-engineered for real use** | Medium | The 8.2 signal (≥15% of relations annotated) is the kill criterion. If it fails, the schema still works — promotion just becomes a power-user feature, not a headline. |
| Graph becomes an unreadable hairball at scale | Medium | Focus mode and overlays are P0, not P1, precisely for this. Default view on open is a *focused* neighborhood, never the whole graph. |
| Users expect sync because everything else has it | Low | Explicit non-goal; SQLite file is portable and the JSON export is lossless. Revisit in v2 with CRDT-backed sync. |

---

## 11. Open questions

1. **Highlight color vs. system color.** Binding them 1:1 is elegant and caps you at ~9 systems before the reader stops being legible. Alternative: systems have colors, highlights have a separate palette, and a node's highlight shows its *primary* system. **Leaning:** 1:1 for the 9 hotkeyed systems, neutral gray for the rest. Needs a real-use test in M2.
2. **Should a node be able to belong to zero systems?** Yes for capture speed (`N` with no system), but unfiled nodes need a home — probably an implicit "Inbox" system. Confirm in M2.
3. **Cross-document nodes.** A node with anchors in three papers is clearly right. Does the *workspace* then need a document-level graph (paper cites paper) as a separate layer? **Leaning:** out of v1 scope; the entity model already permits it.
4. **`same-as` merge semantics.** Auto-merge transitive `same-as` clusters, or keep them distinct and merge only on explicit request? **Leaning:** explicit, with a suggestion badge.
5. **Relation-on-relation depth cap of 4** (I4) is a guess. Instrument it and find the real distribution.
6. **Does the friction scalar earn its place,** or is `|Δ abstraction_level|` sufficient on its own? Ship both in M5, measure whether anyone sets friction manually.

---

## 12. Beyond v1 (sketch, not committed)

- **Local embedding search** over node/relation text and page text — "find where else I was confused about this" — using a small local model, no network.
- **Suggested relations**: surface candidate links from co-occurrence and semantic similarity as *dismissible ghosts*, never auto-created.
- **OCR** for scanned PDFs, unlocking text anchoring there.
- **EPUB and HTML** via the locator variant already allowed for in the anchor model.
- **Spaced repetition over unresolved confusion**: nodes in a "lapse in understanding" system that are still unresolved after *n* days resurface.
- **Resolution tracking**: mark a confusion node resolved, with a link to what resolved it. The graph then shows how understanding was actually built, which is the long-game payoff of the whole model.
- **CRDT sync** across the user's own machines.

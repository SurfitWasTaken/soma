# Soma User Guide

## Before you start

Everything stays in Soma's default workspace, so there is nothing to set up. Launch Soma directly on your paper:

```sh
cd ~/Documents/SlashOS/pdtvty
./scripts/fetch-pdfium.sh            # once: downloads the PDF engine
cargo run --release -p soma-ui -- path/to/paper.pdf
```

Everything you do is saved to the default workspace's `.soma` file (`~/Documents/Soma/workspace.soma`) as you go. There is no save button. Quit and relaunch with the same command to pick up where you left off, including your undo history. You can also open more PDFs later with **Open PDF…** or by dropping a file on the window.

**Ctrl or Cmd.** Every shortcut written as Ctrl-… also works with Cmd on macOS.

### The screen

| Area | What it shows |
| --- | --- |
| Top bar | Open document tabs, Open PDF…, zoom, Fit width / Fit page, Spread, Graph strip toggle, Light / Sepia / Dark, undo / redo, `?` keymap, Canvas toggle |
| Left panel | **Systems** (your classification schemes and overlay switches), **Outline** (the paper's bookmarks), **Docs** (every PDF in the workspace, with relink) |
| Centre | The paper. Continuous scroll; a thin strip on the right edge marks every highlight in the document |
| Right panel | The **graph strip**: the neighbourhood of whatever you last touched, so linking never needs a context switch |
| Bottom bar | Node and relation counts, the workspace path, and a reminder of the most useful keys |

### Systems you start with

| Key | System | Use it for |
| --- | --- | --- |
| Ctrl-1 | lapse in understanding | Anything you do not follow yet |
| Ctrl-2 | terminology | Definitions and names worth pinning down |
| Ctrl-3 | proof obligations | Claims the paper asserts but you would need to check |
| Ctrl-4 | open questions | Things you want to ask or look up later |
| — | inbox | Where unfiled nodes land automatically |

Add your own under **Systems → new system…**. Each new one takes the next free Ctrl-number up to 9.

## Core ideas in two minutes

Soma records the structure of your confusion, not just marks on the page. Five concepts cover everything:

- **Highlight**: a mark on the page with no graph entry (`h`).
- **Node**: a unit of thought. Usually made from a selection, so it keeps a link back to the exact passage.
- **Relation**: a typed link between two things, such as `prerequisite-of` or `contradicts`. A relation is itself a thing: it can have a title and body, join a system, and be linked to.
- **System**: a named classification such as *lapse in understanding*. A node or relation can be in several at once.
- **Overlay**: a view that shows only certain systems. Everything else fades to 12% (ghost mode) or hides.

### Why relations matter

The thought *"I don't see how A leads to B"* belongs on the link, not in A or B. Soma lets you write it there without inventing a fake middle node. The link between A and B stays one hop, so prerequisite chains and layouts stay honest.

A relation shows its state on the canvas:

| State | When | Looks like |
| --- | --- | --- |
| Bare | No text, no system | A plain line, styled by kind |
| Annotated | Has a title or body | A line with a dot at the midpoint |
| Promoted | Filed in a system, or another relation points at it | A diamond on the line; other links can attach to it |

### Relation kinds

| Kind | Direction | Meaning |
| --- | --- | --- |
| prerequisite-of | A → B | Understand A before B. Cycles get a warning |
| elaborates | A → B | B expands on A |
| contradicts | both ways | They can't both be right as stated |
| same-as | both ways | Two names for one thing |
| instance-of | A → B | B is the general case of A |
| depends-on | A → B | Generic dependency |
| unclear-link | both ways | *I don't know how these relate.* Files itself into *lapse in understanding* |

## Scenarios for reading your paper

Each scenario is something you are likely to hit in the first hour, with the exact keys. Selecting text works four ways: drag, double-click a word, triple-click a line, or single-click a word.

### 1. "I don't follow this term"

1. Select the term.
2. Press **Ctrl-1**.

The term is highlighted orange and becomes a node in *lapse in understanding*. The surrounding sentence is saved as its body. A toast confirms it; **Ctrl-Z** undoes. Keep reading.

### 2. "This depends on that earlier thing" (the prerequisite chain)

1. Earlier, you captured term A with **Ctrl-1**.
2. Now select term B and press **Ctrl-1**, then **L**.

That is three keys. Soma links A → B as `prerequisite-of`. For about three seconds a bar at the bottom offers four kinds; press **1–4** to switch kind, or keep reading to accept.

The default kind comes from the system you captured into. *lapse* and *proof obligations* default to `prerequisite-of`; *terminology* defaults to `same-as`; *open questions* to `unclear-link`.

### 3. "This connects to something from pages ago"

1. Capture or click the new thing.
2. Press **l** (lowercase). Yellow two-letter labels appear over nodes in the graph strip.
3. Type a label to link to that node.
4. If the target isn't visible, press **/** and type part of its name, then Enter.

### 4. "Worth marking, not worth a node"

Select and press **h**. It uses the colour of your last capture. No node is created.

### 5. "I want to write my own note on this passage"

1. Select the passage and press **N** (Shift-n).
2. Edit the title and body. Type `[[` in the body to link to another node by name.
3. **Ctrl-Enter** saves; **Esc** cancels.

The node lands in *inbox*. Press **s**, then a digit, to file it into a system.

### 6. "The same idea appears again"

If the new selection has exactly the same text as an existing node's title, **Ctrl-1…9** adds this passage to that node instead of making a duplicate. Otherwise: click the node's existing highlight to focus it, select the new passage, press **a**.

### 7. Figures, tables and equations

Hold **Ctrl** and drag a rectangle around it, then **Ctrl-1…9** or **N**. For an image, a single click often selects the whole object.

### 8. "I get A and I get B, but not the step between them"

This is what Soma is built for. The gap belongs on the link, not in A or B.

1. Link A and B first (scenario 2 or 3).
2. Press **Tab** for the canvas.
3. Click the middle of the A–B line, or click A and press **e** until the line is selected.
4. Press **N**, write the question as the title (e.g. *"why does the limit exist?"*), **Ctrl-Enter**.
5. Press **s**, then **1**, then Enter. The line now shows a diamond: the gap is filed as a lapse.
6. Optional: click another node C, press **l**, and pick the diamond's label. C now points at the gap itself.

A–B is still one relation, one hop apart.

### 9. Review: "what do I actually need to go read?"

1. **Tab** to the canvas, then **Esc** to clear focus.
2. The right panel lists **Start here**: nodes with no prerequisites. These are the roots of your confusion.
3. In the left panel choose **Layered** so chains read top to bottom.
4. Tick only *lapse in understanding* under Overlays. Other entries fade; **G** hides them instead.
5. Click a node and press **f** to see only its neighbourhood. **[** and **]** change the depth.

### 10. Back to the source

- From the canvas: click a node, press **Enter**. The reader jumps to the passage and flashes it.
- From the reader: click any highlight to focus its node. **Ctrl-Enter** opens it in the canvas.

### 11. Theory versus implementation

If the paper moves between levels (maths, algorithm, code), give nodes a level with **Ctrl-Shift-Up/Down** (-3 to +3). Links that cross levels get a dashed overlay. On a focused relation in the canvas, **Ctrl-Shift-1–5** sets how costly that jump is (0.1–0.9). Costly links are drawn longer.

### 12. You download a revised version of the paper

Left panel → **Docs** → **relink…** next to the paper, then pick the new file. Soma re-finds every highlight in the new text and reports how many matched with high confidence. Ones it can only place by position get a red outline.

### 13. Mistakes

**Ctrl-Z** undoes anything, including deletes of whole chains; **Ctrl-Shift-Z** redoes. Undo history survives restarts. **Ctrl-Backspace** deletes the focused item; if more than three things would go, Soma asks first.

## Keymap

Press **F1** in the app for this list. **Tab** switches between reader and canvas; most keys mean the same thing in both.

### Reader

| Key | Action |
| --- | --- |
| Ctrl-1 … Ctrl-9 | Highlight the selection and create a node in system n |
| h | Highlight only, in the last-used colour |
| N | New node from the selection, with a composer (no selection: previous search hit) |
| L | Link the previous node → the node you just created |
| l | Link the current node to a target chosen by hint labels; / to search |
| 1–4 (right after a link) | Change that link's kind |
| a | Add the selection as another passage of the focused node |
| s | File the focused node into systems (digits toggle, Enter closes) |
| Ctrl-drag | Select a rectangle (figure, table, equation) |
| Click a highlight | Focus its node |
| / , n , N | Search the paper, next hit, previous hit |
| j k, arrows, Space, Page Up/Down | Scroll |
| g , G | Top, bottom |
| Alt-Left , Alt-Right | Back, forward (after links, outline jumps, node jumps) |
| Ctrl-+ , Ctrl-− , Ctrl-0 | Zoom in, zoom out, fit width |
| Ctrl-Shift-Up/Down | Raise or lower the focused node's abstraction level |
| Ctrl-Shift-1…9 | Toggle overlay for system n |
| Ctrl-Enter | Open the focused node in the canvas |
| Ctrl-Backspace | Delete the focused item |
| Esc | Clear the selection, then the search |

### Canvas

| Key or mouse | Action |
| --- | --- |
| Click | Focus a node or relation (click the middle of a line for a relation) |
| Ctrl-click two items, then l | Link them |
| h , j , arrows | Move focus to the nearest item in that direction |
| e | Cycle through the focused node's relations |
| Enter | Jump to the source passage in the reader |
| N or double-click | Edit title and body (works on relations too) |
| Double-click empty space, or Ctrl-N | New free-standing node |
| l | Link from the focus, using hint labels |
| L | Link the previously touched item → the focus |
| Drag from a node's edge, or Shift-drag | Draw a link; release on empty space to create a new linked node |
| Drag a node | Move and pin it |
| s | File into systems |
| k | Change a relation's kind |
| Ctrl-r | Reverse a relation |
| Ctrl-Shift-1…5 | Friction 0.1–0.9 on a focused relation (otherwise: toggle overlay n) |
| Ctrl-Shift-Up/Down | Abstraction level |
| f , [ , ] | Focus mode on the focused node; fewer or more hops |
| G | Ghost mode: fade or hide items outside the overlay |
| i | Show only the focused item's system; Esc restores |
| z | Fit the view |
| Scroll, pinch, drag empty space | Zoom and pan |
| Ctrl-Backspace | Delete, with a count when more than 3 items go |
| Esc | Leave isolate, then focus mode, then clear focus |

Both modes: **Ctrl-Z** undo, **Ctrl-Shift-Z** redo, **F1** keymap.

**Canvas filter box** (left panel): plain words search titles and bodies. Add `system:lapse`, `kind:prereq`, `level:1`, `doc:<title>`, or `detached`, `annotated`, `promoted` to narrow further.

## Making the most of Soma

Capture fast while reading and organise later, on the canvas. Habits that pay off:

- **Don't stop to tidy.** Ctrl-1 and keep going. Titles, systems and kinds can all be fixed later with N, s and k.
- **Capture the smallest phrase that names the idea.** A two-word term makes a better node title than a sentence, and the sentence is saved as the body anyway.
- **Link while the connection is fresh.** L right after a capture costs one key. Remembering the link tomorrow costs a re-read.
- **Put doubts on the link, not the node.** When you understand both ideas but not the step between them, annotate the relation (scenario 8). This is the thing no other tool lets you record.
- **Use unclear-link when unsure.** If you know two things relate but not how, press 1–4 after linking and pick `unclear-link`. It files itself as a lapse, so it shows up in review.
- **Keep the graph strip open.** It always shows the neighbourhood of what you just touched, which is where l looks for targets.
- **Make systems for your own questions.** For example *contradicts my prior*, *to verify empirically*, or *worth stealing*. One idea can sit in several.
- **Review with overlays.** A 10-minute pass with only *lapse* on, in Layered view, shows the prerequisite chain and its roots. The roots are what to go read next.
- **Re-read, don't restart.** The workspace keeps everything across sessions, and highlights stay attached even if you replace the PDF with a revised version.

### A suggested session with your paper

1. **First read (30–60 min).** Reader only. Ctrl-1 on every gap, Ctrl-2 on every term, L whenever something clearly depends on the last capture.
2. **Stitch (10 min).** Tab to the canvas. Link stray nodes with l, and set kinds with k.
3. **Name the gaps (10 min).** Find relations you don't really understand, press N and write the question on them. File them with s → 1.
4. **Plan (5 min).** Esc, read the Start here list, and switch to Layered with only *lapse* on. Those roots are your reading list.
5. **Second read.** Click a node, press Enter to jump straight to its passage. Delete or edit nodes as the confusion resolves.

## UAT checklist

Work through these on your paper and record each result. A1–A8 refer to the PRD's acceptance tests.

| # | Check | How | Expected | Result |
| --- | --- | --- | --- | --- |
| 1 | Capture in 2 keys (A1) | Select a term, Ctrl-1 | Orange highlight, toast, node in the strip, counted under *lapse* | |
| 2 | Chain in 3 keys (A2) | On a later page: select, Ctrl-1, L | Kind bar appears; the canvas shows A → B | |
| 3 | Rendering | Scroll the whole paper; zoom to 400% and back; try Fit page and Spread | Sharp text at every zoom, no blank or repeated tiles | |
| 4 | Selection | Drag across two lines; double-click; triple-click; Ctrl-drag a figure | The selection matches what you meant, including in two-column layout | |
| 5 | Search and links | / a term, n / N; click a citation or section link, then Alt-Left | Hits highlighted in order; link jumps and back returns | |
| 6 | Hint linking | l, type a label; l then / and a name | Link created to the chosen node | |
| 7 | Gap on a link (A3) | Scenario 8 in full | Diamond on the line; A and B still directly linked | |
| 8 | Delete and undo (A4) | Ctrl-Backspace on a node with links, confirm, Ctrl-Z | Everything attached disappears, then all returns | |
| 9 | Jump both ways | Canvas: Enter on a node. Reader: click a highlight | Reader flashes the passage; the canvas focuses the node | |
| 10 | Review views | Only *lapse* on; Layered; f on a node; Esc for Start here | Chain reads top-down; roots listed | |
| 11 | Persistence (A7) | Quit mid-session (or force-quit), relaunch | Nothing lost; Ctrl-Z still undoes the last action | |
| 12 | Revised PDF (A5) | If you have another version: Docs → relink… | Most highlights land on the same words | |

### Known limitations in this build

- A selection can't span two pages.
- Scanned PDFs without a text layer only support rectangle (Ctrl-drag) anchors.
- Hint labels only cover nodes visible in the graph strip or canvas. Use / for anything else.
- Not built yet: saved views (keys 1–9 in the canvas), the friction and abstraction lenses, Markdown and GraphML export.
- Keyboard input methods (IME) and screen readers are untested.
- Very dense graphs (hundreds of nodes) are only tested synthetically.

### Reporting an issue

For each failure, note the check number, the page of the paper, the keys you pressed, and what you saw. A screenshot helps. To check the workspace itself is healthy, run:

```sh
cargo run --release -p soma-cli -- verify ~/Documents/Soma/workspace.soma
cargo run --release -p soma-cli -- stats ~/Documents/Soma/workspace.soma
```

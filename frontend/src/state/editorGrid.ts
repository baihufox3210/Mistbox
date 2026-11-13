// Editor groups for the IDE layout (like VS Code): the editor area is a tree of
// splits (rows / columns) whose leaves are groups, each with its own tabs.
// A file is open in at most one group. Pure functions; the layout keeps the
// state and reconciles it with the open tabs owned by `useDrive`.

export type Dir = 'row' | 'col';
export type Edge = 'left' | 'right' | 'top' | 'bottom';

export type GNode = { t: 'g'; id: string } | { t: 's'; id: string; dir: Dir; kids: GNode[]; sizes: number[] };

export interface Group {
  tabs: string[];
  active: string | null;
}

export interface Grid {
  root: GNode;
  groups: Record<string, Group>;
  focused: string;
}

let seq = 0;
const nextId = (p: string) => `${p}${++seq}`;

export function initialGrid(): Grid {
  return { root: { t: 'g', id: 'g0' }, groups: { g0: { tabs: [], active: null } }, focused: 'g0' };
}

/** Group ids in visual order (left→right, top→bottom). */
export function groupOrder(n: GNode): string[] {
  return n.t === 'g' ? [n.id] : n.kids.flatMap(groupOrder);
}

export function groupOf(g: Grid, tab: string): string | null {
  for (const [id, gr] of Object.entries(g.groups)) if (gr.tabs.includes(tab)) return id;
  return null;
}

const dirOf = (e: Edge): Dir => (e === 'left' || e === 'right' ? 'row' : 'col');
const after = (e: Edge) => e === 'right' || e === 'bottom';

/** Put a new group `nid` beside group `target`, on side `edge`. */
function insertBeside(root: GNode, target: string, nid: string, edge: Edge): GNode {
  const ng: GNode = { t: 'g', id: nid };
  const dir = dirOf(edge);
  const walk = (n: GNode): GNode => {
    if (n.t === 'g') {
      if (n.id !== target) return n;
      return { t: 's', id: nextId('s'), dir, kids: after(edge) ? [n, ng] : [ng, n], sizes: [0.5, 0.5] };
    }
    const i = n.kids.findIndex((k) => k.t === 'g' && k.id === target);
    if (i >= 0 && n.dir === dir) {
      // Same direction: add a sibling instead of nesting another split.
      const kids = [...n.kids];
      const sizes = [...n.sizes];
      const half = sizes[i] / 2;
      sizes[i] = half;
      const at = after(edge) ? i + 1 : i;
      kids.splice(at, 0, ng);
      sizes.splice(at, 0, half);
      return { ...n, kids, sizes };
    }
    return { ...n, kids: n.kids.map(walk) };
  };
  return walk(root);
}

/** Remove a group leaf; collapses splits left with a single child. */
function removeLeaf(root: GNode, id: string): GNode | null {
  const walk = (n: GNode): GNode | null => {
    if (n.t === 'g') return n.id === id ? null : n;
    const kids: GNode[] = [];
    const sizes: number[] = [];
    n.kids.forEach((k, i) => {
      const r = walk(k);
      if (!r) return;
      // A collapsed child split in the same direction merges into this one.
      if (r.t === 's' && r.dir === n.dir) {
        r.kids.forEach((kk, j) => (kids.push(kk), sizes.push(n.sizes[i] * r.sizes[j])));
      } else {
        kids.push(r);
        sizes.push(n.sizes[i]);
      }
    });
    if (kids.length === 0) return null;
    if (kids.length === 1) return kids[0];
    const sum = sizes.reduce((a, b) => a + b, 0);
    return { ...n, kids, sizes: sizes.map((s) => s / sum) };
  };
  return walk(root);
}

/** Drop a group from the layout (its tabs go with it; callers close them first). */
export function removeGroup(g: Grid, id: string): Grid {
  if (Object.keys(g.groups).length <= 1) return g;
  const root = removeLeaf(g.root, id) ?? g.root;
  const groups = { ...g.groups };
  delete groups[id];
  const order = groupOrder(root);
  return { root, groups, focused: g.focused === id ? order[0] : g.focused };
}

/** Remove `tab` from its group; the group's active tab moves to a neighbour. */
function detach(g: Grid, tab: string): Grid {
  const from = groupOf(g, tab);
  if (!from) return g;
  const gr = g.groups[from];
  const i = gr.tabs.indexOf(tab);
  const tabs = gr.tabs.filter((t) => t !== tab);
  const active = gr.active === tab ? (tabs[Math.min(i, tabs.length - 1)] ?? null) : gr.active;
  return { ...g, groups: { ...g.groups, [from]: { tabs, active } } };
}

function attach(g: Grid, tab: string, to: string, index?: number): Grid {
  const gr = g.groups[to];
  const tabs = [...gr.tabs];
  tabs.splice(index ?? tabs.length, 0, tab);
  return { ...g, groups: { ...g.groups, [to]: { tabs, active: tab } }, focused: to };
}

/** Drop groups that lost their last tab (never the only group). */
function pruneEmptied(g: Grid, touched: Iterable<string>): Grid {
  let out = g;
  for (const id of touched) if (out.groups[id] && out.groups[id].tabs.length === 0) out = removeGroup(out, id);
  return out;
}

/** Move an open tab into group `to` (at `index`, default: end) and focus it. */
export function moveTab(g: Grid, tab: string, to: string, index?: number): Grid {
  const from = groupOf(g, tab);
  if (from === to && index !== undefined) {
    // Reorder within the group.
    const cur = g.groups[to].tabs.indexOf(tab);
    const at = cur < index ? index - 1 : index;
    const tabs = g.groups[to].tabs.filter((t) => t !== tab);
    tabs.splice(at, 0, tab);
    return { ...g, groups: { ...g.groups, [to]: { tabs, active: tab } }, focused: to };
  }
  if (from === to) return focusTab(g, tab);
  const out = attach(detach(g, tab), tab, to, index);
  return from ? pruneEmptied(out, [from]) : out;
}

/** New group beside `target`; with `tab`, the tab moves there. Focuses the new group. */
export function split(g: Grid, target: string, edge: Edge, tab?: string): { grid: Grid; group: string } {
  const nid = nextId('g');
  let out: Grid = {
    root: insertBeside(g.root, target, nid, edge),
    groups: { ...g.groups, [nid]: { tabs: [], active: null } },
    focused: nid,
  };
  if (tab) {
    const from = groupOf(out, tab);
    out = attach(detach(out, tab), tab, nid);
    if (from) out = pruneEmptied(out, [from]);
  }
  return { grid: out, group: nid };
}

export function focusTab(g: Grid, tab: string): Grid {
  const id = groupOf(g, tab);
  if (!id) return g;
  return { ...g, groups: { ...g.groups, [id]: { ...g.groups[id], active: tab } }, focused: id };
}

export function focusGroup(g: Grid, id: string): Grid {
  return g.groups[id] ? { ...g, focused: id } : g;
}

/** Remove a group, moving its tabs into the next group (no tab is closed). */
export function mergeGroup(g: Grid, id: string): Grid {
  const order = groupOrder(g.root);
  if (order.length <= 1 || !g.groups[id]) return g;
  const i = order.indexOf(id);
  const into = order[i + 1] ?? order[i - 1];
  const moved = g.groups[id].tabs;
  const target = g.groups[into];
  const out = removeGroup({ ...g, groups: { ...g.groups, [id]: { tabs: [], active: null } } }, id);
  return {
    ...out,
    groups: { ...out.groups, [into]: { tabs: [...target.tabs, ...moved], active: target.active ?? moved[0] ?? null } },
    focused: into,
  };
}

/** Every tab back into one group. */
export function joinAll(g: Grid): Grid {
  const order = groupOrder(g.root);
  const first = order[0];
  const tabs = order.flatMap((id) => g.groups[id].tabs);
  const active = g.groups[g.focused]?.active ?? tabs[0] ?? null;
  return { root: { t: 'g', id: first }, groups: { [first]: { tabs, active } }, focused: first };
}

export function setSizes(g: Grid, splitId: string, sizes: number[]): Grid {
  const walk = (n: GNode): GNode => (n.t === 'g' ? n : n.id === splitId ? { ...n, sizes } : { ...n, kids: n.kids.map(walk) });
  return { ...g, root: walk(g.root) };
}

/**
 * Bring the grid in line with the open tabs:
 * - closed tabs leave their group (groups emptied this way disappear);
 * - newly opened tabs go to `place` (a group, optionally split on an edge)
 *   or the focused group;
 * - the globally active tab's group becomes focused.
 * Returns the tab that should be globally active: when the active tab was just
 * closed, it is the neighbour *in the same group* (not in the global list).
 */
export function reconcile(
  g: Grid,
  open: string[],
  active: string | null,
  prevActive: string | null,
  place?: { group: string; edge?: Edge } | null,
): { grid: Grid; active: string | null } {
  const openSet = new Set(open);
  let out = g;
  const touched = new Set<string>();
  let closedActive = false;
  for (const [id, gr] of Object.entries(g.groups)) {
    for (const t of gr.tabs) {
      if (openSet.has(t)) continue;
      if (t === prevActive && id === g.focused) closedActive = true;
      out = detach(out, t);
      touched.add(id);
    }
  }
  const focusedBefore = out.focused;
  out = pruneEmptied(out, touched);
  for (const t of open) {
    if (groupOf(out, t)) continue;
    let to = out.focused;
    if (place && out.groups[place.group]) {
      if (place.edge) {
        const r = split(out, place.group, place.edge);
        out = r.grid;
        to = r.group;
      } else to = place.group;
      place = null;
    }
    out = attach(out, t, to);
  }
  let want = active;
  if (closedActive) {
    // The focused group's own neighbour wins over useDrive's global pick;
    // if that group is gone, keep the global pick.
    const fg = out.groups[focusedBefore];
    want = fg ? fg.active : active && groupOf(out, active) ? active : (out.groups[out.focused]?.active ?? null);
  }
  if (want && groupOf(out, want)) out = focusTab(out, want);
  return { grid: out, active: want };
}

import { describe, expect, it } from 'vitest';
import { groupOf, groupOrder, initialGrid, joinAll, mergeGroup, moveTab, reconcile, split, type Grid } from './editorGrid';

function open(tabs: string[], active: string | null = tabs[tabs.length - 1] ?? null): Grid {
  return reconcile(initialGrid(), tabs, active, null).grid;
}

describe('editor grid', () => {
  it('opens tabs in the focused group', () => {
    const g = open(['a', 'b']);
    expect(g.groups.g0).toEqual({ tabs: ['a', 'b'], active: 'b' });
  });

  it('splits right with the active tab and nests / flattens splits', () => {
    let g = open(['a', 'b']);
    const r = split(g, 'g0', 'right', 'b');
    g = r.grid;
    expect(groupOrder(g.root)).toEqual(['g0', r.group]);
    expect(g.groups.g0).toEqual({ tabs: ['a'], active: 'a' });
    expect(g.groups[r.group]).toEqual({ tabs: ['b'], active: 'b' });
    expect(g.focused).toBe(r.group);
    // Same direction → sibling, not a nested split.
    const r2 = split(g, r.group, 'right');
    expect(g.root.t === 's' && r2.grid.root.t === 's' && r2.grid.root.kids.length).toBe(3);
    // Other direction → nested column split.
    const r3 = split(g, 'g0', 'bottom');
    expect(r3.grid.root.t === 's' && r3.grid.root.kids[0].t).toBe('s');
    expect(groupOrder(r3.grid.root)).toEqual(['g0', r3.group, r.group]);
  });

  it('removes a group when its last tab leaves, collapsing the split', () => {
    let g = open(['a', 'b']);
    const r = split(g, 'g0', 'right', 'b');
    g = moveTab(r.grid, 'b', 'g0');
    expect(g.root).toEqual({ t: 'g', id: 'g0' });
    expect(g.groups.g0.tabs).toEqual(['a', 'b']);
    // Splitting the only tab out of a group replaces that group.
    const r2 = split(open(['a']), 'g0', 'left', 'a');
    expect(groupOrder(r2.grid.root)).toEqual([r2.group]);
  });

  it('closing the active tab keeps focus in its own group', () => {
    let g = open(['a', 'b', 'c']);
    g = split(g, 'g0', 'right', 'c').grid; // g0: a b | new: c
    g = moveTab(g, 'b', 'g0');
    g = reconcile(g, ['a', 'b', 'c'], 'b', 'c').grid;
    expect(groupOf(g, 'b')).toBe('g0');
    // useDrive picks "c" (global neighbour) after closing "b"; we want "a".
    const r = reconcile(g, ['a', 'c'], 'c', 'b');
    expect(r.active).toBe('a');
    expect(r.grid.focused).toBe('g0');
    // Closing the last tab of a group drops the group; global pick stands.
    const r2 = reconcile(r.grid, ['c'], 'c', 'a');
    expect(groupOrder(r2.grid.root).length).toBe(1);
    expect(r2.active).toBe('c');
  });

  it('places new tabs at a drop target, splitting when an edge is given', () => {
    const g = open(['a']);
    const r = reconcile(g, ['a', 'x'], 'x', 'a', { group: 'g0', edge: 'bottom' });
    expect(groupOrder(r.grid.root).length).toBe(2);
    expect(groupOf(r.grid, 'x')).not.toBe('g0');
    expect(r.grid.focused).toBe(groupOf(r.grid, 'x'));
  });

  it('reorders within a group, merges and joins', () => {
    let g = open(['a', 'b', 'c']);
    g = moveTab(g, 'c', 'g0', 0);
    expect(g.groups.g0.tabs).toEqual(['c', 'a', 'b']);
    g = moveTab(g, 'c', 'g0', 3);
    expect(g.groups.g0.tabs).toEqual(['a', 'b', 'c']);
    const r = split(g, 'g0', 'right', 'c');
    const m = mergeGroup(r.grid, r.group);
    expect(groupOrder(m.root)).toEqual(['g0']);
    expect(m.groups.g0.tabs).toEqual(['a', 'b', 'c']);
    const r2 = split(split(g, 'g0', 'right', 'c').grid, 'g0', 'bottom', 'b');
    const j = joinAll(r2.grid);
    expect(groupOrder(j.root)).toEqual(['g0']);
    expect(j.groups.g0.tabs.sort()).toEqual(['a', 'b', 'c']);
  });
});

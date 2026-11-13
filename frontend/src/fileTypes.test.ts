import { describe, expect, it } from 'vitest';
import { languageFor, previewKind } from './fileTypes';
import { formatSize, joinPath } from './format';
import { languageExtension } from './preview/CodeView';

describe('preview routing', () => {
  it('maps every required extension to a highlighter', () => {
    const want: Record<string, string> = {
      'a.c': 'c', 'a.h': 'c', 'a.cpp': 'cpp', 'a.hpp': 'cpp', 'A.java': 'java', 'a.py': 'python', 'a.rs': 'rust',
      'a.js': 'javascript', 'a.ts': 'typescript', 'a.tsx': 'tsx', 'a.json': 'json', 'a.yaml': 'yaml', 'a.toml': 'toml',
      'a.sh': 'shell', 'a.sql': 'sql', 'a.txt': 'text', 'a.log': 'text', 'Makefile': 'shell',
    };
    for (const [name, lang] of Object.entries(want)) {
      expect(previewKind(name), name).toBe('code');
      expect(languageFor(name), name).toBe(lang);
      if (lang !== 'text') expect(languageExtension(lang), name).not.toEqual([]);
    }
  });

  it('distinguishes markdown, latex, media, pdf, binary and unknown', () => {
    expect(previewKind('README.md')).toBe('markdown');
    expect(previewKind('paper.tex')).toBe('latex');
    expect(previewKind('a.PNG')).toBe('image');
    expect(previewKind('a.svg')).toBe('image');
    expect(previewKind('a.pdf')).toBe('pdf');
    expect(previewKind('a.mp3')).toBe('audio');
    expect(previewKind('a.webm')).toBe('video');
    expect(previewKind('a.zip')).toBe('binary');
    expect(previewKind('noext')).toBe('unknown'); // tries text, falls back to info if binary
    expect(previewKind('.hidden')).toBe('unknown');
    expect(previewKind('data.xyz')).toBe('unknown');
  });

  it('formats helpers', () => {
    expect(formatSize(0)).toBe('0 B');
    expect(formatSize(1536)).toBe('1.5 KB');
    expect(joinPath('/a/b', '../c/./d.md')).toBe('/a/c/d.md');
  });
});

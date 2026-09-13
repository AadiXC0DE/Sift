import { describe, it, expect, afterEach } from 'vitest';
import StarterKit from '@tiptap/starter-kit';
import { Editor } from '@tiptap/react';
import {
  SignatureBlock,
  applySignature,
  composedBodyHtml,
  findSignatureRange,
  signatureInsertPos,
  signatureSource,
  wrapSignature,
} from './signature';

const editors: Editor[] = [];

function makeEditor(html: string): Editor {
  const editor = new Editor({
    extensions: [StarterKit.configure({ heading: false }), SignatureBlock],
    content: html,
  });
  editors.push(editor);
  return editor;
}

afterEach(() => {
  while (editors.length) editors.pop()?.destroy();
});

describe('signature source', () => {
  const accounts = [
    { id: 'a1', signature_html: '  <p>Sig One</p>  ' },
    { id: 'a2', signature_html: null },
  ];

  it('reads the account signature and treats blank as none', () => {
    expect(signatureSource(accounts, 'a1')).toBe('<p>Sig One</p>');
    expect(signatureSource(accounts, 'a2')).toBe('');
    expect(signatureSource(accounts, 'missing')).toBe('');
  });

  it('wraps only a non-empty signature in the marked block', () => {
    expect(wrapSignature('<p>x</p>')).toContain('data-sift-signature="1"');
    expect(wrapSignature('')).toBe('');
  });
});

describe('P5.5 signature switching after typing', () => {
  it('replaces only the marked block and keeps the user text', () => {
    const editor = makeEditor(composedBodyHtml('<p>Sig One</p>'));
    editor.commands.insertContentAt(1, 'Hello typed');

    applySignature(editor, '<p>Sig Two</p>');

    const html = editor.getHTML();
    expect(html).toContain('Hello typed');
    expect(html).toContain('Sig Two');
    expect(html).not.toContain('Sig One');
    expect(html.match(/data-sift-signature/g)).toHaveLength(1);
  });

  it('does not duplicate the signature when applied twice', () => {
    const editor = makeEditor(composedBodyHtml('<p>Sig One</p>'));
    applySignature(editor, '<p>Sig One</p>');
    applySignature(editor, '<p>Sig One</p>');
    expect(editor.getHTML().match(/data-sift-signature/g)).toHaveLength(1);
  });

  it('inserts after the leading paragraph when the body has no signature', () => {
    const editor = makeEditor('<p>Hello</p>');
    applySignature(editor, '<p>Sig Two</p>');
    const html = editor.getHTML();
    expect(html).toContain('Hello');
    expect(html).toContain('Sig Two');
    expect(html.indexOf('Hello')).toBeLessThan(html.indexOf('Sig Two'));
  });

  it('removes the block when the new account has no signature and keeps text', () => {
    const editor = makeEditor(composedBodyHtml('<p>Sig One</p>'));
    editor.commands.insertContentAt(1, 'Hello typed');

    applySignature(editor, '');

    const html = editor.getHTML();
    expect(html).toContain('Hello typed');
    expect(html).not.toContain('Sig One');
    expect(html).not.toContain('data-sift-signature');
  });

  it('reverts a signature switch as one undo step', () => {
    const editor = makeEditor(composedBodyHtml('<p>Sig One</p>'));
    editor.commands.insertContentAt(1, 'Hello typed');

    applySignature(editor, '<p>Sig Two</p>');
    expect(editor.getHTML()).toContain('Sig Two');

    editor.commands.undo();

    const html = editor.getHTML();
    expect(html).toContain('Sig One');
    expect(html).not.toContain('Sig Two');
    expect(html).toContain('Hello typed');
  });
});

describe('signature ranges', () => {
  it('finds the marked block and its insert position', () => {
    const editor = makeEditor(composedBodyHtml('<p>Sig</p>'));
    const range = findSignatureRange(editor.state.doc);
    expect(range).toEqual({ from: 2, to: editor.state.doc.content.size });
    expect(signatureInsertPos(editor.state.doc)).toBe(2);
  });

  it('reports no range for a body without a signature', () => {
    const editor = makeEditor('<p>Hello</p>');
    expect(findSignatureRange(editor.state.doc)).toBeNull();
    expect(signatureInsertPos(editor.state.doc)).toBe(7);
  });
});

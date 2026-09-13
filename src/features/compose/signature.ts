import { Node, type Editor } from '@tiptap/react';
import type { Account } from '../../app/ipc/types';

export const SIGNATURE_ATTR = 'data-sift-signature';

/**
 * The one block Sift owns inside a composed body. Marking it lets a From
 * change replace exactly the signature and never the user's typed text.
 */
export const SignatureBlock = Node.create({
  name: 'siftSignature',
  group: 'block',
  content: 'block+',
  defining: true,
  parseHTML: () => [{ tag: `div[${SIGNATURE_ATTR}]` }],
  renderHTML: () => ['div', { [SIGNATURE_ATTR]: '1' }, 0],
});

/** The account's raw signature HTML, or '' when none is configured. */
export function signatureSource(
  accounts: Pick<Account, 'id' | 'signature_html'>[],
  accountId: string,
): string {
  return accounts.find((a) => a.id === accountId)?.signature_html?.trim() ?? '';
}

/** A signature wrapped in the marked block, ready to insert into the body. */
export function wrapSignature(raw: string): string {
  return raw ? `<div ${SIGNATURE_ATTR}="1">${raw}</div>` : '';
}

/** Body template: typing paragraph, then the signature, then any quote. */
export function composedBodyHtml(raw: string, quoteHtml = ''): string {
  return `<p></p>${wrapSignature(raw)}${quoteHtml}`;
}

type PmDoc = Editor['state']['doc'];

/** Range of the marked signature block, if the body currently has one. */
export function findSignatureRange(doc: PmDoc): { from: number; to: number } | null {
  const hits: { from: number; to: number }[] = [];
  doc.descendants((node, pos) => {
    if (hits.length) return false;
    if (node.type.name === 'siftSignature') hits.push({ from: pos, to: pos + node.nodeSize });
    return true;
  });
  return hits[0] ?? null;
}

/**
 * Where a first signature goes: directly after the leading block (the empty
 * paragraph the composer starts with), so it sits below typed text and above
 * any quote instead of at the very end of the document.
 */
export function signatureInsertPos(doc: PmDoc): number {
  return doc.firstChild ? doc.firstChild.nodeSize : 0;
}

/**
 * Apply `raw` as the body's signature. Replaces the marked block in place when
 * one exists, inserts it after the leading block otherwise, and removes it
 * when the account has no signature. One transaction, so undo reverts it as a
 * single step.
 */
export function applySignature(editor: Editor, raw: string): void {
  const range = findSignatureRange(editor.state.doc);
  if (range) {
    if (raw) {
      editor
        .chain()
        .insertContentAt({ from: range.from + 1, to: range.to - 1 }, raw)
        .run();
    } else if (editor.state.doc.childCount > 1) {
      editor.chain().deleteRange(range).run();
    } else {
      editor
        .chain()
        .insertContentAt({ from: range.from + 1, to: range.to - 1 }, '')
        .run();
    }
    return;
  }
  if (raw) editor.chain().insertContentAt(signatureInsertPos(editor.state.doc), wrapSignature(raw)).run();
}

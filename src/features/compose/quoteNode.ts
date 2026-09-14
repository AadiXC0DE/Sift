import { Node } from '@tiptap/react';

export const QUOTE_ATTR = 'data-sift-quote';
export const QUOTE_ATTRIBUTION_ATTR = 'data-attribution';

/**
 * The quoted original message (P5.4).
 *
 * A reply keeps the whole original body in the document even when the drawer is
 * collapsed: the content lives in a real node, so what the composer sends is
 * what the reader saw. Representing it as a node (rather than letting TipTap
 * flatten the markup) is what makes the collapse honest.
 */
export const QuoteBlock = Node.create({
  name: 'siftQuote',
  group: 'block',
  content: 'block+',
  defining: true,
  addAttributes: () => ({
    attribution: {
      default: '',
      parseHTML: (el) => el.getAttribute(QUOTE_ATTRIBUTION_ATTR) ?? '',
      renderHTML: (attrs) => ({ [QUOTE_ATTRIBUTION_ATTR]: attrs.attribution as string }),
    },
  }),
  parseHTML: () => [{ tag: `details[${QUOTE_ATTR}]`, contentElement: 'blockquote' }],
  renderHTML: ({ node, HTMLAttributes }) => [
    'details',
    { class: 'sift-quote', [QUOTE_ATTR]: '1', ...HTMLAttributes },
    ['summary', {}, node.attrs.attribution as string],
    ['blockquote', {}, 0],
  ],
});

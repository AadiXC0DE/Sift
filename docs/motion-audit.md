# Motion audit (10.5)

| Component | Before | After | Status |
|---|---|---|---|
| j/k focus | none | none | pass |
| view switch | none | none | pass |
| hover | bg 80ms | bg 80ms | pass |
| tooltip | 125ms ease-out | 125ms ease-out | pass |
| menu/popover | 160ms from origin | 160ms from origin | pass |
| dialog | 200ms center | 200ms center | pass |
| sheet | 260ms drawer + drag spring | same | pass |
| toast | 320ms + undo | same | pass |
| star | 180ms 1->1.25->1 | same | pass |
| send morph | 200ms blur crossfade | same | pass |
| reduced-motion | transforms removed | verified | pass |

Zero open rows.

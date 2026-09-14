import React from 'react';
import { api } from '../../app/ipc/commands';
import type { Label } from '../../app/ipc/types';
import { Button } from '../../ui/Button';
import { Dialog } from '../../ui/Dialog';
import { Segmented } from '../../ui/Segmented';
import { Switch } from '../../ui/Switch';
import {
  utilities,
  type LabelWithParent,
  type MailRule,
  type MailRuleInput,
  type RuleAction,
  type RuleActionKind,
  type RuleCondition,
  type RuleField,
  type RuleOp,
  type RulePreview,
} from '../mail-utilities/ipc';

/**
 * The compact local rules editor (P8.3).
 *
 * Every rule this panel writes is one of the documented conditions and actions
 * below — there is no script, regex or URL action surface, and the editor has
 * no field that could express one. A rule never deletes mail: the actions only
 * add a label, archive, mark read, star or move to Junk, and running an edited
 * rule over existing mail is always previewed with its match count first.
 */

const FIELD_LABEL: Record<RuleField, string> = {
  sender: 'Sender',
  recipient: 'Recipient',
  subject: 'Subject',
  hasAttachment: 'Has attachment',
};

/** The operator surface per field; the editor never offers another one. */
const OPS_BY_FIELD: Record<RuleField, RuleOp[]> = {
  sender: ['contains', 'is', 'domain'],
  recipient: ['contains', 'is'],
  subject: ['contains'],
  hasAttachment: ['isTrue'],
};

const OP_LABEL: Record<RuleOp, string> = {
  contains: 'contains',
  is: 'is',
  domain: 'domain is',
  isTrue: 'is true',
};

const ACTION_LABEL: Record<RuleActionKind, string> = {
  addLabel: 'Add label',
  archive: 'Archive',
  markRead: 'Mark as read',
  star: 'Star',
  junk: 'Move to Junk',
};

const FIELD_ORDER: RuleField[] = ['sender', 'recipient', 'subject', 'hasAttachment'];
const ACTION_ORDER: RuleActionKind[] = ['addLabel', 'archive', 'markRead', 'star', 'junk'];

const inputStyle: React.CSSProperties = {
  height: 28,
  border: '1px solid var(--border-strong)',
  borderRadius: 6,
  padding: '0 8px',
  background: 'var(--bg-raised)',
  color: 'var(--fg)',
  fontSize: 12.5,
};

/** `invoke` rejects with the backend's message, which is not always an `Error`. */
function errorText(err: unknown): string {
  return err instanceof Error ? err.message : String(err);
}

/**
 * The label tree depth is only sent by builds that project parents (P8.5).
 * Reading it through this guard keeps the indentation honest instead of
 * inventing a level when the field is absent.
 */
function labelDepth(label: Label): number {
  if (!('depth' in label)) return 0;
  const { depth } = label as LabelWithParent;
  return typeof depth === 'number' ? depth : 0;
}

/** The one-line row summary: what the rule matches, then what it does. */
function ruleSummary(rule: MailRule, labelName: (id: string) => string): string {
  const conditions = rule.conditions.map((c) =>
    c.op === 'isTrue' ? FIELD_LABEL[c.field] : `${FIELD_LABEL[c.field]} ${OP_LABEL[c.op]} "${c.value}"`,
  );
  const actions = rule.actions.map((a) =>
    a.kind === 'addLabel'
      ? `Add label "${a.labelId ? labelName(a.labelId) : 'label'}"`
      : ACTION_LABEL[a.kind],
  );
  const match =
    conditions.length === 0 ? 'No conditions' : conditions.join(rule.match === 'all' ? ' and ' : ' or ');
  return `${match} → ${actions.length === 0 ? 'no actions' : actions.join(', ')}`;
}

/**
 * The `MailRuleInput` for a persisted rule. Preview, apply and the enabled
 * switch all send exactly what the backend stores, so an update can never
 * rewrite a field the user did not touch.
 */
function ruleInput(rule: MailRule, patch: Partial<MailRuleInput> = {}): MailRuleInput {
  return {
    id: rule.id,
    accountId: rule.accountId,
    name: rule.name,
    enabled: rule.enabled,
    match: rule.match,
    conditions: rule.conditions,
    actions: rule.actions,
    sortOrder: rule.sortOrder,
    ...patch,
  };
}

interface EditorState {
  accountId: string;
  /** `null` while creating; the stored id while editing. */
  id: string | null;
  /** `expectedRevision` for an edit; `null` means the rule does not exist yet. */
  revision: number | null;
  sortOrder: number | null;
  enabled: boolean;
  name: string;
  match: 'all' | 'any';
  conditions: RuleCondition[];
  actions: RuleAction[];
  saving: boolean;
  error: string | null;
}

/**
 * A rule is storable only when it names itself, matches something and does
 * something. `hasAttachment` carries no value; every other operator needs one
 * to match anything at all.
 */
function editorIsValid(draft: EditorState): boolean {
  if (draft.name.trim().length === 0) return false;
  if (draft.conditions.length === 0 || draft.actions.length === 0) return false;
  if (draft.conditions.some((c) => c.op !== 'isTrue' && c.value.trim().length === 0)) return false;
  if (draft.actions.some((a) => a.kind === 'addLabel' && !a.labelId)) return false;
  return true;
}

interface PreviewState {
  accountId: string;
  rule: MailRule;
  status: 'checking' | 'ready' | 'applying' | 'applied' | 'error';
  preview: RulePreview | null;
  error: string | null;
  applied: number;
  skipped: number;
}

const NEVER_DELETES_COPY =
  'This only changes labels and folders for the messages it matches. Sift never deletes mail here.';

export function RulesPanel({
  accountIds,
  accountNames,
}: {
  accountIds: string[];
  accountNames?: Record<string, string>;
}) {
  const [rules, setRules] = React.useState<Record<string, MailRule[] | undefined>>({});
  const [labels, setLabels] = React.useState<Record<string, Label[] | undefined>>({});
  const [loadErrors, setLoadErrors] = React.useState<Record<string, string | undefined>>({});
  const [labelErrors, setLabelErrors] = React.useState<Record<string, string | undefined>>({});
  const [rowErrors, setRowErrors] = React.useState<Record<string, string | undefined>>({});
  const [busy, setBusy] = React.useState<Record<string, boolean>>({});
  const [confirming, setConfirming] = React.useState<string | null>(null);
  const [editing, setEditing] = React.useState<EditorState | null>(null);
  const [preview, setPreview] = React.useState<PreviewState | null>(null);
  const alive = React.useRef(true);
  React.useEffect(() => {
    // The app mounts under StrictMode, which runs cleanups and re-mounts the
    // effects: re-arm here or the first load's answer is discarded.
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const idsRef = React.useRef(accountIds);
  idsRef.current = accountIds;
  // The effect keys off the account *set*, not the array identity: a caller
  // that hands back a fresh array each render must not refetch forever.
  const accountKey = accountIds.join('|');

  const loadAccount = React.useCallback(async (accountId: string) => {
    const [rulesResult, labelsResult] = await Promise.allSettled([
      utilities.rules_list({ accountId }),
      api.labels_list(accountId),
    ]);
    if (!alive.current) return;
    if (rulesResult.status === 'fulfilled') {
      const rows = rulesResult.value;
      setRules((cur) => ({ ...cur, [accountId]: rows }));
      setLoadErrors((cur) => ({ ...cur, [accountId]: undefined }));
    } else {
      // The stored rows stay on screen; the section reports why it is stale.
      setRules((cur) => ({ ...cur, [accountId]: cur[accountId] ?? [] }));
      setLoadErrors((cur) => ({ ...cur, [accountId]: errorText(rulesResult.reason) }));
    }
    if (labelsResult.status === 'fulfilled') {
      const rows = labelsResult.value;
      setLabels((cur) => ({ ...cur, [accountId]: rows }));
      setLabelErrors((cur) => ({ ...cur, [accountId]: undefined }));
    } else {
      setLabels((cur) => ({ ...cur, [accountId]: cur[accountId] ?? [] }));
      setLabelErrors((cur) => ({ ...cur, [accountId]: errorText(labelsResult.reason) }));
    }
  }, []);

  React.useEffect(() => {
    for (const accountId of idsRef.current) void loadAccount(accountId);
  }, [accountKey, loadAccount]);

  const startCreate = (accountId: string) => {
    setEditing({
      accountId,
      id: null,
      revision: null,
      sortOrder: null,
      // Rules are off until the user turns them on (P8.3): saving a rule must
      // never start acting on incoming mail by itself.
      enabled: false,
      name: '',
      match: 'all',
      conditions: [{ field: 'sender', op: 'contains', value: '' }],
      actions: [],
      saving: false,
      error: null,
    });
  };

  const startEdit = (rule: MailRule) => {
    setEditing({
      accountId: rule.accountId,
      id: rule.id,
      revision: rule.revision,
      sortOrder: rule.sortOrder,
      enabled: rule.enabled,
      name: rule.name,
      match: rule.match,
      conditions: rule.conditions.map((c) => ({ ...c })),
      actions: rule.actions.map((a) => ({ ...a })),
      saving: false,
      error: null,
    });
  };

  const saveEditor = async () => {
    if (!editing) return;
    const accountId = editing.accountId;
    const input: MailRuleInput = {
      id: editing.id,
      accountId,
      name: editing.name.trim(),
      enabled: editing.enabled,
      match: editing.match,
      conditions: editing.conditions.map((c) => ({
        field: c.field,
        op: c.op,
        value: c.op === 'isTrue' ? '' : c.value.trim(),
      })),
      actions: editing.actions.map((a) => ({
        kind: a.kind,
        labelId: a.kind === 'addLabel' ? a.labelId : null,
      })),
      sortOrder: editing.sortOrder ?? undefined,
    };
    setEditing((cur) => (cur ? { ...cur, saving: true, error: null } : cur));
    try {
      await utilities.rules_upsert({
        rule: input,
        expectedRevision: editing.revision ?? undefined,
      });
      await loadAccount(accountId);
      if (alive.current) setEditing(null);
    } catch (err) {
      // The draft keeps its text so nothing typed is lost to a rejected save.
      if (alive.current) {
        setEditing((cur) => (cur ? { ...cur, saving: false, error: errorText(err) } : cur));
      }
    }
  };

  const toggleEnabled = async (rule: MailRule, enabled: boolean) => {
    setRowErrors((cur) => ({ ...cur, [rule.id]: undefined }));
    const applyEnabled = (value: boolean) =>
      setRules((cur) => ({
        ...cur,
        [rule.accountId]: (cur[rule.accountId] ?? []).map((r) =>
          r.id === rule.id ? { ...r, enabled: value } : r,
        ),
      }));
    // Flip immediately so the switch answers the click, then reconcile with the
    // stored revision the upsert answers with.
    applyEnabled(enabled);
    setBusy((cur) => ({ ...cur, [rule.id]: true }));
    try {
      await utilities.rules_upsert({
        rule: ruleInput(rule, { enabled }),
        expectedRevision: rule.revision,
      });
      await loadAccount(rule.accountId);
    } catch (err) {
      applyEnabled(rule.enabled);
      if (alive.current) setRowErrors((cur) => ({ ...cur, [rule.id]: errorText(err) }));
    } finally {
      if (alive.current) setBusy((cur) => ({ ...cur, [rule.id]: false }));
    }
  };

  const deleteRule = async (rule: MailRule) => {
    setConfirming(null);
    setRowErrors((cur) => ({ ...cur, [rule.id]: undefined }));
    setBusy((cur) => ({ ...cur, [rule.id]: true }));
    try {
      await utilities.rules_delete({ accountId: rule.accountId, ruleId: rule.id });
      await loadAccount(rule.accountId);
    } catch (err) {
      if (alive.current) setRowErrors((cur) => ({ ...cur, [rule.id]: errorText(err) }));
    } finally {
      if (alive.current) setBusy((cur) => ({ ...cur, [rule.id]: false }));
    }
  };

  const openPreview = async (rule: MailRule) => {
    setPreview({
      accountId: rule.accountId,
      rule,
      status: 'checking',
      preview: null,
      error: null,
      applied: 0,
      skipped: 0,
    });
    try {
      // The count comes from the backend before any message is touched.
      const result = await utilities.rules_preview({
        accountId: rule.accountId,
        rule: ruleInput(rule),
      });
      if (!alive.current) return;
      setPreview((cur) =>
        cur && cur.rule.id === rule.id ? { ...cur, status: 'ready', preview: result } : cur,
      );
    } catch (err) {
      if (!alive.current) return;
      setPreview((cur) =>
        cur && cur.rule.id === rule.id ? { ...cur, status: 'error', error: errorText(err) } : cur,
      );
    }
  };

  const applyPreview = async () => {
    const current = preview;
    if (!current || current.status !== 'ready' || !current.preview) return;
    if (current.preview.count === 0) return;
    setPreview({ ...current, status: 'applying' });
    try {
      const result = await utilities.rules_apply_existing({
        accountId: current.accountId,
        ruleId: current.rule.id,
        revision: current.rule.revision,
      });
      if (!alive.current) return;
      setPreview((cur) =>
        cur && cur.rule.id === current.rule.id
          ? { ...cur, status: 'applied', applied: result.applied, skipped: result.skipped }
          : cur,
      );
    } catch (err) {
      if (!alive.current) return;
      setPreview((cur) =>
        cur && cur.rule.id === current.rule.id ? { ...cur, status: 'error', error: errorText(err) } : cur,
      );
    }
  };

  if (accountIds.length === 0) {
    return <p style={{ fontSize: 13, color: 'var(--fg-2)' }}>No accounts are connected.</p>;
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 14 }}>
      <p style={{ margin: 0, fontSize: 12, color: 'var(--fg-3)' }}>
        Rules run on mail as it arrives in Sift, on this device.
      </p>
      {accountIds.map((accountId) => {
        const accountName = accountNames?.[accountId] ?? accountId;
        const rows = rules[accountId];
        const loadError = loadErrors[accountId];
        return (
          <section
            key={accountId}
            aria-label={`${accountName} rules`}
            style={{
              border: '1px solid var(--border)',
              borderRadius: 'var(--r-md)',
              padding: '10px 12px',
              background: 'var(--n1)',
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <h3 style={{ margin: 0, fontSize: 13, fontWeight: 600, flex: 1 }}>{accountName}</h3>
              <Button size="sm" variant="ghost" onClick={() => startCreate(accountId)}>
                New rule
              </Button>
            </div>

            {loadError !== undefined ? (
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 8 }}>
                <span style={{ fontSize: 12, color: 'var(--danger)', flex: 1 }}>{loadError}</span>
                <Button size="sm" onClick={() => void loadAccount(accountId)}>
                  Retry
                </Button>
              </div>
            ) : rows === undefined ? (
              <p style={{ margin: '8px 0 0', fontSize: 12.5, color: 'var(--fg-3)' }}>Loading rules…</p>
            ) : rows.length === 0 ? (
              <p style={{ margin: '8px 0 0', fontSize: 12.5, color: 'var(--fg-3)' }}>
                No rules for this account yet.
              </p>
            ) : (
              <ul style={{ listStyle: 'none', margin: '4px 0 0', padding: 0 }}>
                {rows.map((rule, index) => {
                  const error = rowErrors[rule.id] ?? rule.lastError;
                  const isBusy = busy[rule.id] === true;
                  return (
                    <li
                      key={rule.id}
                      data-rule-id={rule.id}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        gap: 8,
                        padding: '8px 0',
                        borderTop: index === 0 ? 'none' : '1px solid var(--border)',
                      }}
                    >
                      <div style={{ flex: 1, minWidth: 0 }}>
                        <div style={{ fontSize: 13, fontWeight: 500 }}>{rule.name}</div>
                        <div
                          style={{
                            fontSize: 12,
                            color: 'var(--fg-3)',
                            whiteSpace: 'nowrap',
                            overflow: 'hidden',
                            textOverflow: 'ellipsis',
                          }}
                        >
                          {ruleSummary(
                            rule,
                            (id) => (labels[accountId] ?? []).find((l) => l.id === id)?.name ?? id,
                          )}
                        </div>
                        {error && (
                          <div style={{ fontSize: 12, color: 'var(--danger)', marginTop: 2 }}>{error}</div>
                        )}
                      </div>
                      <Button
                        size="sm"
                        variant="ghost"
                        disabled={isBusy}
                        onClick={() => void openPreview(rule)}
                      >
                        Run on existing mail…
                      </Button>
                      <Button size="sm" variant="ghost" disabled={isBusy} onClick={() => startEdit(rule)}>
                        Edit
                      </Button>
                      {confirming === rule.id ? (
                        <>
                          <span style={{ fontSize: 12, color: 'var(--fg-3)' }}>
                            Delete this rule? No mail is deleted.
                          </span>
                          <Button
                            size="sm"
                            variant="danger"
                            aria-label={`Confirm delete ${rule.name}`}
                            disabled={isBusy}
                            onClick={() => void deleteRule(rule)}
                          >
                            Delete
                          </Button>
                          <Button
                            size="sm"
                            variant="ghost"
                            aria-label={`Cancel delete ${rule.name}`}
                            onClick={() => setConfirming(null)}
                          >
                            Cancel
                          </Button>
                        </>
                      ) : (
                        <Button
                          size="sm"
                          variant="ghost"
                          aria-label={`Delete rule ${rule.name}`}
                          disabled={isBusy}
                          onClick={() => setConfirming(rule.id)}
                        >
                          Delete
                        </Button>
                      )}
                      <Switch
                        checked={rule.enabled}
                        ariaLabel={`${rule.name} enabled`}
                        onChange={(value) => void toggleEnabled(rule, value)}
                      />
                    </li>
                  );
                })}
              </ul>
            )}

            {editing && editing.accountId === accountId && (
              <RuleEditor
                draft={editing}
                labels={labels[accountId] ?? []}
                labelsError={labelErrors[accountId]}
                onChange={setEditing}
                onSave={() => void saveEditor()}
                onCancel={() => setEditing(null)}
              />
            )}
          </section>
        );
      })}

      {preview && (
        <RunOnExistingDialog
          state={preview}
          onApply={() => void applyPreview()}
          onClose={() => setPreview(null)}
        />
      )}
    </div>
  );
}

function RuleEditor({
  draft,
  labels,
  labelsError,
  onChange,
  onSave,
  onCancel,
}: {
  draft: EditorState;
  labels: Label[];
  labelsError: string | undefined;
  onChange: (draft: EditorState) => void;
  onSave: () => void;
  onCancel: () => void;
}) {
  const setCondition = (index: number, patch: Partial<RuleCondition>) =>
    onChange({
      ...draft,
      conditions: draft.conditions.map((c, i) => (i === index ? { ...c, ...patch } : c)),
    });
  const setAction = (index: number, patch: Partial<RuleAction>) =>
    onChange({
      ...draft,
      actions: draft.actions.map((a, i) => (i === index ? { ...a, ...patch } : a)),
    });

  return (
    <div
      style={{
        marginTop: 10,
        border: '1px solid var(--border-strong)',
        borderRadius: 'var(--r-md)',
        padding: 12,
        display: 'flex',
        flexDirection: 'column',
        gap: 10,
        background: 'var(--bg-raised)',
      }}
    >
      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <input
          aria-label="Rule name"
          value={draft.name}
          placeholder="Rule name"
          onChange={(e) => onChange({ ...draft, name: e.target.value })}
          style={{ ...inputStyle, flex: 1, height: 30 }}
        />
      </div>

      {draft.id === null && (
        <p style={{ margin: 0, fontSize: 12, color: 'var(--fg-3)' }}>
          New rules are saved off. Switch it on in the list when you want it to run on incoming mail.
        </p>
      )}

      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <span style={{ fontSize: 12.5, color: 'var(--fg-2)' }}>Match</span>
        <Segmented
          value={draft.match}
          onChange={(value) => onChange({ ...draft, match: value })}
          options={[
            { value: 'all', label: 'All' },
            { value: 'any', label: 'Any' },
          ]}
        />
        <span style={{ fontSize: 12.5, color: 'var(--fg-2)' }}>
          {draft.match === 'all' ? 'of these conditions' : 'condition'}
        </span>
      </div>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <span style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--fg-2)' }}>Conditions</span>
        {draft.conditions.map((c, index) => (
          <div key={index} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <select
              aria-label="Condition field"
              value={c.field}
              onChange={(e) => {
                const field = e.target.value as RuleField;
                // The operator belongs to the field: switching fields clears both
                // it and the value rather than carrying a meaningless one over.
                setCondition(index, { field, op: OPS_BY_FIELD[field][0], value: '' });
              }}
              style={inputStyle}
            >
              {FIELD_ORDER.map((field) => (
                <option key={field} value={field}>
                  {FIELD_LABEL[field]}
                </option>
              ))}
            </select>
            <select
              aria-label="Condition operator"
              value={c.op}
              onChange={(e) => setCondition(index, { op: e.target.value as RuleOp })}
              style={inputStyle}
            >
              {OPS_BY_FIELD[c.field].map((op) => (
                <option key={op} value={op}>
                  {OP_LABEL[op]}
                </option>
              ))}
            </select>
            {c.op !== 'isTrue' && (
              <input
                aria-label="Condition value"
                value={c.value}
                placeholder="Value"
                onChange={(e) => setCondition(index, { value: e.target.value })}
                style={{ ...inputStyle, flex: 1, minWidth: 120 }}
              />
            )}
            <Button
              size="sm"
              variant="ghost"
              aria-label={`Remove condition ${index + 1}`}
              onClick={() =>
                onChange({
                  ...draft,
                  conditions: draft.conditions.filter((_, i) => i !== index),
                })
              }
            >
              Remove
            </Button>
          </div>
        ))}
        <div>
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              onChange({
                ...draft,
                conditions: [...draft.conditions, { field: 'sender', op: 'contains', value: '' }],
              })
            }
          >
            Add condition
          </Button>
        </div>
      </div>

      <div style={{ display: 'flex', flexDirection: 'column', gap: 6 }}>
        <span style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--fg-2)' }}>Actions</span>
        {draft.actions.map((a, index) => (
          <div key={index} style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
            <select
              aria-label="Action"
              value={a.kind}
              onChange={(e) => {
                const kind = e.target.value as RuleActionKind;
                // Only an addLabel action carries a label.
                setAction(index, { kind, labelId: kind === 'addLabel' ? a.labelId : null });
              }}
              style={inputStyle}
            >
              {ACTION_ORDER.map((kind) => (
                <option key={kind} value={kind}>
                  {ACTION_LABEL[kind]}
                </option>
              ))}
            </select>
            {a.kind === 'addLabel' && (
              <select
                aria-label="Label"
                value={a.labelId ?? ''}
                onChange={(e) => setAction(index, { labelId: e.target.value || null })}
                style={{ ...inputStyle, flex: 1, minWidth: 140 }}
              >
                <option value="">Select a label</option>
                {labels.map((label) => (
                  <option
                    key={label.id}
                    value={label.id}
                    data-depth={labelDepth(label)}
                    style={{ paddingLeft: 8 + labelDepth(label) * 12 }}
                  >
                    {label.name}
                  </option>
                ))}
              </select>
            )}
            <Button
              size="sm"
              variant="ghost"
              aria-label={`Remove action ${index + 1}`}
              onClick={() => onChange({ ...draft, actions: draft.actions.filter((_, i) => i !== index) })}
            >
              Remove
            </Button>
          </div>
        ))}
        <div>
          <Button
            size="sm"
            variant="ghost"
            onClick={() =>
              onChange({ ...draft, actions: [...draft.actions, { kind: 'archive', labelId: null }] })
            }
          >
            Add action
          </Button>
        </div>
        {labelsError !== undefined && (
          <span style={{ fontSize: 12, color: 'var(--danger)' }}>
            Labels could not be loaded: {labelsError}
          </span>
        )}
      </div>

      <div style={{ display: 'flex', gap: 8, alignItems: 'center' }}>
        <Button size="sm" variant="primary" disabled={draft.saving || !editorIsValid(draft)} onClick={onSave}>
          {draft.saving ? 'Saving…' : 'Save'}
        </Button>
        <Button size="sm" variant="ghost" disabled={draft.saving} onClick={onCancel}>
          Cancel
        </Button>
        {draft.error && <span style={{ fontSize: 12, color: 'var(--danger)', flex: 1 }}>{draft.error}</span>}
      </div>
    </div>
  );
}

function RunOnExistingDialog({
  state,
  onApply,
  onClose,
}: {
  state: PreviewState;
  onApply: () => void;
  onClose: () => void;
}) {
  const { rule, status, preview } = state;
  const count = preview?.count ?? 0;
  return (
    <Dialog open onClose={onClose} title={`Run "${rule.name}" on existing mail`} width={520}>
      {status === 'checking' && (
        <p style={{ margin: '0 0 12px', fontSize: 13, color: 'var(--fg-2)' }}>
          Checking how many existing messages match…
        </p>
      )}

      {status === 'error' && (
        <p style={{ margin: '0 0 12px', fontSize: 13, color: 'var(--danger)' }}>{state.error}</p>
      )}

      {status === 'ready' && count === 0 && (
        <p style={{ margin: '0 0 12px', fontSize: 13 }}>
          No existing messages match this rule, so there is nothing to apply.
        </p>
      )}

      {status === 'ready' && count > 0 && (
        <>
          <p style={{ margin: '0 0 8px', fontSize: 13 }}>
            {count === 1 ? '1 existing message matches.' : `${count} existing messages match.`}
          </p>
          <ul style={{ listStyle: 'none', margin: '0 0 10px', padding: 0 }}>
            {preview?.sample.slice(0, 3).map((row) => (
              <li
                key={row.threadId}
                style={{
                  padding: '6px 0',
                  borderTop: '1px solid var(--border)',
                  fontSize: 12.5,
                }}
              >
                <div style={{ fontWeight: 500 }}>{row.subject}</div>
                <div style={{ color: 'var(--fg-3)' }}>
                  {row.fromName ? `${row.fromName} <${row.fromEmail}>` : row.fromEmail}
                </div>
              </li>
            ))}
          </ul>
        </>
      )}

      {status === 'applying' && (
        <p style={{ margin: '0 0 12px', fontSize: 13, color: 'var(--fg-2)' }}>Applying…</p>
      )}

      {status === 'applied' && (
        <p style={{ margin: '0 0 12px', fontSize: 13 }}>
          Applied to {state.applied} {state.applied === 1 ? 'message' : 'messages'}.
          {state.skipped > 0 ? ` ${state.skipped} were skipped.` : ''}
        </p>
      )}

      <p style={{ margin: '0 0 12px', fontSize: 12, color: 'var(--fg-3)' }}>{NEVER_DELETES_COPY}</p>

      <div style={{ display: 'flex', gap: 8, justifyContent: 'flex-end' }}>
        {status !== 'applied' && (
          <Button size="sm" variant="ghost" onClick={onClose}>
            Skip this rule
          </Button>
        )}
        {status === 'ready' && count > 0 && (
          <Button size="sm" variant="primary" onClick={onApply}>
            {`Apply to ${count} ${count === 1 ? 'message' : 'messages'}`}
          </Button>
        )}
        {status === 'applied' && (
          <Button size="sm" variant="primary" onClick={onClose}>
            Close
          </Button>
        )}
      </div>
    </Dialog>
  );
}

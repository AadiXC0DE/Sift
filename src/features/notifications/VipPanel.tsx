import React from 'react';
import { toast } from 'sonner';
import { utilities } from '../mail-utilities/ipc';
// `Contact` is not re-exported by the utilities barrel; it is the existing
// shared DTO `vip_candidates` answers with.
import type { Contact } from '../../app/ipc/types';
import { Button } from '../../ui/Button';

interface AccountVips {
  /** `null` until the first answer for that account arrives. */
  vips: string[] | null;
  candidates: Contact[] | null;
  failed: boolean;
}

const EMPTY: AccountVips = { vips: null, candidates: null, failed: false };

/**
 * P8.4 — VIP senders per account.
 *
 * The candidate list comes from mail Sift has already seen (`vip_candidates`),
 * so choosing a VIP never triggers a Contacts permission prompt. Every change
 * re-renders from the list the backend returns rather than guessing.
 */
export function VipPanel({
  accountIds,
  accountNames,
}: {
  accountIds: string[];
  accountNames?: Record<string, string>;
}) {
  const [byAccount, setByAccount] = React.useState<Record<string, AccountVips>>({});

  // Keyed by content, not array identity: a parent that builds the id list
  // inline would otherwise refetch on every render.
  const key = accountIds.join('\u0000');
  React.useEffect(() => {
    const ids = key === '' ? [] : key.split('\u0000');
    let alive = true;
    for (const accountId of ids) {
      Promise.all([utilities.vip_list({ accountId }), utilities.vip_candidates({ accountId, limit: 20 })])
        .then(([vips, candidates]) => {
          if (alive) setByAccount((prev) => ({ ...prev, [accountId]: { vips, candidates, failed: false } }));
        })
        .catch(() => {
          // One account failing must not blank the others.
          if (alive) {
            setByAccount((prev) => ({ ...prev, [accountId]: { vips: [], candidates: [], failed: true } }));
          }
        });
    }
    return () => {
      alive = false;
    };
  }, [key]);

  const setVip = async (accountId: string, email: string, vip: boolean) => {
    try {
      const vips = await utilities.vip_set({ accountId, email, vip });
      setByAccount((prev) => ({ ...prev, [accountId]: { ...(prev[accountId] ?? EMPTY), vips } }));
    } catch {
      toast.error(vip ? `Could not add ${email} as a VIP` : `Could not remove ${email} from VIPs`);
    }
  };

  if (accountIds.length === 0) {
    return (
      <div role="status" style={{ fontSize: 12, color: 'var(--fg-3)' }}>
        Add an account before choosing VIP senders.
      </div>
    );
  }

  return (
    <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
      {accountIds.map((accountId) => {
        const data = byAccount[accountId] ?? EMPTY;
        const vips = data.vips ?? [];
        const candidates = data.candidates ?? [];
        // Anyone already a VIP drops out of the suggestions, so the two lists
        // can never show the same address with opposite actions.
        const suggested = candidates.filter(
          (c) => !vips.some((v) => v.toLowerCase() === c.email.toLowerCase()),
        );
        return (
          <div
            key={accountId}
            data-vip-account={accountId}
            style={{
              display: 'flex',
              flexDirection: 'column',
              gap: 6,
              border: '1px solid var(--border)',
              borderRadius: 'var(--r-md)',
              padding: '10px 12px',
            }}
          >
            <div style={{ fontSize: 13, fontWeight: 500 }}>{accountNames?.[accountId] ?? accountId}</div>
            {data.failed && (
              <div role="status" style={{ fontSize: 12, color: 'var(--fg-3)' }}>
                VIP senders are not available for this account right now.
              </div>
            )}
            {!data.failed && data.vips === null && (
              <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>Loading VIP senders…</div>
            )}
            {!data.failed && data.vips !== null && (
              <>
                <div style={{ fontSize: 11, color: 'var(--fg-3)', textTransform: 'uppercase' }}>VIPs</div>
                {vips.length === 0 ? (
                  <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
                    No VIP senders yet — add one from the list below.
                  </div>
                ) : (
                  vips.map((email) => {
                    const name = candidates.find((c) => c.email.toLowerCase() === email.toLowerCase())?.name;
                    return (
                      <div
                        key={email}
                        data-vip={email}
                        style={{
                          display: 'flex',
                          alignItems: 'center',
                          justifyContent: 'space-between',
                          gap: 8,
                        }}
                      >
                        <span style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
                          {name && <span style={{ fontSize: 13 }}>{name}</span>}
                          <span
                            style={{ fontSize: name ? 11 : 13, color: name ? 'var(--fg-3)' : 'var(--fg)' }}
                          >
                            {email}
                          </span>
                        </span>
                        <Button
                          size="sm"
                          variant="ghost"
                          aria-label={`Remove ${email}`}
                          onClick={() => void setVip(accountId, email, false)}
                        >
                          Remove
                        </Button>
                      </div>
                    );
                  })
                )}
                <div style={{ fontSize: 11, color: 'var(--fg-3)', textTransform: 'uppercase', marginTop: 4 }}>
                  Recent correspondents
                </div>
                {suggested.length === 0 ? (
                  <div style={{ fontSize: 12, color: 'var(--fg-3)' }}>
                    Nobody to suggest yet — Sift lists people you have recently exchanged mail with, and never
                    reads your Contacts.
                  </div>
                ) : (
                  suggested.map((c) => (
                    <div
                      key={c.email}
                      data-vip-candidate={c.email}
                      style={{
                        display: 'flex',
                        alignItems: 'center',
                        justifyContent: 'space-between',
                        gap: 8,
                      }}
                    >
                      <span style={{ display: 'flex', flexDirection: 'column', minWidth: 0 }}>
                        {c.name && <span style={{ fontSize: 13 }}>{c.name}</span>}
                        <span
                          style={{ fontSize: c.name ? 11 : 13, color: c.name ? 'var(--fg-3)' : 'var(--fg)' }}
                        >
                          {c.email}
                        </span>
                      </span>
                      <Button
                        size="sm"
                        aria-label={`Add ${c.email} as a VIP`}
                        onClick={() => void setVip(accountId, c.email, true)}
                      >
                        Add
                      </Button>
                    </div>
                  ))
                )}
              </>
            )}
          </div>
        );
      })}
    </div>
  );
}

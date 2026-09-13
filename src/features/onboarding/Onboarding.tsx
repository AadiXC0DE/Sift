import React, { useEffect } from 'react';
import { useAccounts } from '../../stores/accountsStore';
import { useSetup } from '../../stores/setupStore';
import { Wizard } from './Wizard';

export function Onboarding({ force = false, onClose }: { force?: boolean; onClose?: () => void }) {
  const accounts = useAccounts((s) => s.accounts);
  const refresh = useAccounts((s) => s.refresh);
  const reset = useSetup((s) => s.reset);

  useEffect(() => {
    void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (accounts.length > 0 && !force) reset();
  }, [accounts.length, force, reset]);

  const demoForce = useSetup((s) => s.demoForce);
  if (accounts.length > 0 && !demoForce && !force) return null;
  return (
    <Wizard
      onDone={() => {
        void refresh();
        onClose?.();
      }}
    />
  );
}

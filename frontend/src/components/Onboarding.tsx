// No project yet: connect the first repository (ConnectRepo).

import { ConnectRepo } from './ConnectRepo';

export function Onboarding({ connected }: { connected: boolean }) {
  return (
    <div className="onboarding">
      <h1>Lobotomy</h1>
      <ConnectRepo connected={connected} />
    </div>
  );
}

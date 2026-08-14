/**
 * Real-browser harness for the deferred tooltip root.
 *
 * jsdom cannot drive Kobalte's tooltip far enough to render content, so the
 * unit tests can only assert that the root exists. The behaviour that actually
 * matters — a tooltip appears on the first hover, via the pointer event that
 * is replayed into the freshly mounted trigger — needs a browser with real
 * pointer input. This mounts the production path: `Button` with a `label`,
 * which is what wraps ~800 call sites in the app.
 */
// Relative, not aliased: tsconfig scopes path aliases to ./src, and this
// fixture deliberately lives outside it.
import { render } from 'solid-js/web';
import { Button } from '../../../../src/components/ui/components/Button';

function Harness() {
  return (
    <main style={{ padding: '80px', display: 'flex', gap: '24px' }}>
      <Button label="Reply" data-testid="hover-target">
        Reply
      </Button>
      <Button label="Delete" data-testid="focus-target">
        Delete
      </Button>
      <Button label="Nope" tooltipDisabled data-testid="disabled-target">
        Nope
      </Button>
    </main>
  );
}

const root = document.getElementById('root');
if (!root) throw new Error('missing #root');
render(() => <Harness />, root);

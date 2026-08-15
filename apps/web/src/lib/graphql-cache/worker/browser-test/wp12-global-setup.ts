import { rm } from 'node:fs/promises';
import { resolve } from 'node:path';

/** Stale partial evidence must never survive into a WP-12 measurement run. */
export default async function wp12GlobalSetup(): Promise<void> {
  const directory = resolve(
    import.meta.dirname,
    '../../../../../measurements/generated'
  );
  await Promise.all(
    ['chromium-production', 'firefox-production'].map((project) =>
      rm(resolve(directory, `cache-wasm-wp12-${project}.json`), {
        force: true,
      })
    )
  );
}

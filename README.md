# Upgrader for Horizon Android (alpha source)

This project is a client-side prototype for Horizon's sandboxed `.hmod` code-mod API. It is **not a compiled or tested release**.

## What it currently does
- Opens a separate menu using `/upgrader` or the U key.
- Lists available recipe-registry items and shows estimated values.
- Calculates the original default odds formula: `clamp(0.9 * offer_value / target_value, 0.001, 0.9)`.
- Runs a 70-tick wheel wait, then submits a main-hand replacement/removal command.
- Caps target quantities for common unstackable and 16-stack item families to avoid invalid item counts.
- Cancels the pending attempt if the held stack changes during the wheel.
- Refuses to start unless the main hand contains exactly one offer item, to reduce the risk of erasing a whole stack.

## Limitations you must understand
- The SDK has no general API to atomically read/write a survival inventory or to perform a server-authoritative inventory transaction. This version only targets the main-hand stack through vanilla `/item replace` commands. Cheats/operator rights may be required. The one-item safety lock reduces stack loss but does not make command execution atomic.
- Command execution may be rejected by the server. Check Minecraft command feedback.
- The value table is only a recipe-based estimate with fallback values. It does not reproduce the original mod's full recipe/loot-table sampling, configured overrides, multipliers, or durability discount.
- The wheel's local pseudo-random roll is not a server-secure random source. Do not use this on a competitive server or with valuable items.
- This is an alpha source project; compile and in-game testing have not been completed in this environment.

## Build in GitHub Actions
### Build from an Android phone
1. Download and extract this ZIP on your phone.
2. Create an empty repository on GitHub.
3. Upload the **contents inside** the `upgrader-hmod` folder to the repository root. Keep `.github/workflows/build.yml` in place; enable showing hidden files if your file picker hides `.github`.
4. Open the repository's **Actions** tab and select **Build Upgrader HMOD** → **Run workflow** (or push files to `main`).
5. Open the completed run and download the `upgrader-hmod` artifact ZIP. Extract it to obtain the `.hmod` file.

The workflow runs Rust unit tests, builds and validates the `.hmod`, and uploads it as an artifact if successful. A green build still does not replace in-game testing.

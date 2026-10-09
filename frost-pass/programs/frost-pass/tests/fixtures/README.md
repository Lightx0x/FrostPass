# Test Fixtures

## `mpl_core_program.so`

- **Source:** Official release artifact from the Metaplex Core repository:
  [`https://github.com/metaplex-foundation/mpl-core/releases/tag/release%2Fcore%400.15.3`](https://github.com/metaplex-foundation/mpl-core/releases/tag/release%2Fcore%400.15.3)
- **Direct Download URL:**
  `https://github.com/metaplex-foundation/mpl-core/releases/download/release/core@0.15.3/mpl_core_program.so`
- **Program ID:** `CoREENxT6tWduKuWWydGRfdKgidcgTueqm6nvqm32hs`
- **Version:** `0.15.3` (pre-compiled SBF binary)

### How to update in the future:
To update to a newer release of Metaplex Core:
```bash
curl -L -o programs/frost-pass/tests/fixtures/mpl_core_program.so \
  https://github.com/metaplex-foundation/mpl-core/releases/download/release%2Fcore%40<NEW_VERSION>/mpl_core_program.so
```
Or dump directly from devnet/mainnet if deployed:
```bash
solana program dump -u m CoREENxT6tWduKuWWydGRfdKgidcgTueqm6nvqm32hs programs/frost-pass/tests/fixtures/mpl_core_program.so
```

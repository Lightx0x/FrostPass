# Test Fixtures

## `mpl_core_program.so`

- **Source:** Deployed binary dump from Solana Devnet / Mainnet (`CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`), trimmed to exact ELF size.
- **Program ID:** `CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d`
- **ProgramData Address:** `9ZC25KLUrfgSoFVgjE1rrydZBbZns58UXi8A8ZhTdGfr`
- **Size:** 810,288 bytes
- **SHA-256 Checksum:** `515541c84caa71a2f142c603fc1e2372196825003e66126d1e824dd9213ea056`

### How to update in the future:
Dump directly from devnet/mainnet and trim zero padding down to the ELF size (`e_shoff + e_shnum * e_shentsize`):
```bash
solana program dump -u d CoREENxT6tW1HoK8ypY1SxRMZTcVPm7R94rH4PZNhX7d /tmp/mpl_core_dump.so
head -c 810288 /tmp/mpl_core_dump.so > programs/frost-pass/tests/fixtures/mpl_core_program.so
```

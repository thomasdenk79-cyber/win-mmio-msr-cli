# TODO

Open work in order. Each item names its evidence; tick it only with that evidence in the
validation report (copy of [VALIDATION_REPORT_TEMPLATE.md](VALIDATION_REPORT_TEMPLATE.md)).

## Evaluation on the target device (test signing, owner approval)

- [x] E1 Record the system identity (template section 1). Captured in
      [EVALUATION.md](EVALUATION.md); re-record after the reinstall. Device identifiers stay out of
      the public repo.
- [x] E2 Test-sign and register the default read-only driver with
      [scripts/eval-testsign.ps1](../scripts/eval-testsign.ps1) (done 2026-11-14, CI build `9b8cefd`).
      Lost by the planned reinstall; repeat per [EVALUATION.md](EVALUATION.md).
- [ ] E2a Owner reboots the test device so that test signing takes effect.
- [ ] E3 `sc.exe start rapl-pkg`; `win-mmio-msr-cli status --bridge <eval dir>\rapl-bridge.exe`.
      Evidence: output, `sc.exe query rapl-pkg`, Code Integrity events (Microsoft-Windows-CodeIntegrity/Operational).
- [ ] E4 Compare MSR `0x606`/`0x610` three times with HWiNFO or ThrottleStop (template section 4).
- [ ] E5 Note which power tools are active (for example Intel IPF/DTT, vendor tools); they may
      override PL1/PL2.
- [ ] E6 Build with `RaplEnableMmioRead=1`, test-sign, collect the MCHBAR evidence (template
      section 3) and three read-only comparisons of `MCHBAR+0x59A0`.
- [ ] E7 Independent review of E1–E6 before any write build.
- [ ] E8 Build with `RaplMmioLayoutValidated=1` and `RaplEnableLiveWrites=1`; `apply` a small
      change with `--audit-log`, check readback, rollback and audit records (template section 5–6).
- [ ] E9 Watchdog with a low correction budget (template section 7).
- [ ] E10 Thermal and stability run, WHEA check (template section 8).
- [ ] E11 Remove the evaluation setup with [scripts/eval-rollback.ps1](../scripts/eval-rollback.ps1)
      when evaluation ends; reboot.

## Open questions

- [ ] Q1 Does HVCI or a WDAC policy on the device block the test-signed driver? Answered by E3.
- [ ] Q2 How do existing tools such as ThrottleStop reach MSR and MCHBAR, and which signing route do
      they use? Document in ARCHITECTURE_REVIEW.md as a comparison.

## Repository

- [ ] P1 Review and merge the PR for branch `eval/test-signing` (git process minimal).

## Release

- [ ] R1 Microsoft attestation signing (Partner Center account and EV certificate required; not
      available yet).
- [ ] R2 Replace `HalGetBusDataByOffset` with a PnP filter on the host bridge.
- [ ] R3 Installer uninstall switch.
- [ ] R4 Reviewed `docs/VALIDATION_REPORT.md`; only then any readiness claim.

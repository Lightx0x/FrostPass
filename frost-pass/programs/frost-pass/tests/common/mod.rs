use litesvm::LiteSVM;
use solana_keypair::Keypair;
use solana_signer::Signer;

pub struct TestContext {
    pub svm: LiteSVM,
    pub organizer: Keypair,
}

impl TestContext {
    pub fn new() -> Self {
        let mut svm = LiteSVM::new();

        // Load Metaplex Core program binary (official release v0.15.3)
        let mpl_core_bytes = std::fs::read("tests/fixtures/mpl_core_program.so")
            .or_else(|_| std::fs::read("programs/frost-pass/tests/fixtures/mpl_core_program.so"))
            .expect(
                "Failed to read mpl_core_program.so fixture at tests/fixtures/mpl_core_program.so",
            );
        svm.add_program(mpl_core::ID, &mpl_core_bytes)
            .expect("Failed to load Metaplex Core program into LiteSVM");

        // Load FrostPass program binary
        let frost_pass_bytes = std::fs::read("../../target/deploy/frost_pass.so")
            .or_else(|_| std::fs::read("target/deploy/frost_pass.so"))
            .expect(
                "Failed to read frost_pass.so binary. Ensure you run 'cargo build-sbf' or 'anchor build' before running tests",
            );
        svm.add_program(frost_pass::ID, &frost_pass_bytes)
            .expect("Failed to load FrostPass program into LiteSVM");

        let organizer = Keypair::new();
        svm.airdrop(&organizer.pubkey(), 10_000_000_000).unwrap();

        Self { svm, organizer }
    }
}

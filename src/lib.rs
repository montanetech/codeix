pub mod cli;
pub mod index;
pub mod mount;
pub mod parser;
pub mod server;
pub mod utils;
pub mod watcher;

#[cfg(test)]
mod gitignore_tests {
    use ignore::gitignore::GitignoreBuilder;
    use std::path::Path;

    #[test]
    fn test_target_ignored() {
        let root = Path::new(".");
        let mut builder = GitignoreBuilder::new(root);
        builder.add(".gitignore");
        let gi = builder.build().unwrap();

        let rel_path = "target/.rustc_info.json";

        // matched() - what handler.rs uses
        let m1 = gi.matched(rel_path, false);
        // matched_path_or_any_parents() - checks parent dirs too
        let m2 = gi.matched_path_or_any_parents(rel_path, false);

        println!("matched: {:?}", m1);
        println!("matched_path_or_any_parents: {:?}", m2);

        assert!(
            m2.is_ignore(),
            "target/ files should be ignored via matched_path_or_any_parents"
        );
    }
}

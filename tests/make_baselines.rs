//! Records the dialogue tests' expected results from the code it is built with.
//!
//! The `test_merged_dialogue_*` tests (integration_tests.rs) merge a plugin into a master
//! and compare every topic's response links with `./ignore/<case>/Expect.rkyv`. Those
//! files are greatness7's and are not in the repository. This writes them from whatever
//! merge_to_master it is compiled against: run it on **upstream**
//! (Greatness7/merge_to_master, e.g. 5ea27f1) to record upstream's answers, then run this
//! fork's tests against them - a check that the fork merges dialogue exactly as upstream
//! does.
//!
//! For each of `MW`, `MW_TB`, `MW_BM` and `MW_TB_BM` that has `Master.esm` and
//! `Plugin.esp` in `./ignore/<case>/` (with the plugin's own masters beside them, under
//! their real names, since merging loads them from the same folder), it merges them as
//! that case's test does. `TB_BM` (`Morrowind.esm`, `Tribunal.esm`, `Bloodmoon.esm` in
//! `./ignore/TB_BM/`) is checked against `MW_TB_BM`'s file; when `MW_TB_BM` has no pair of
//! its own, that file is written from the TB_BM chain instead. Cases without their files
//! are skipped and said so.
//!
//! `ignore/` is gitignored; the game's files are never committed. Ignored by default:
//!
//! ```text
//! cargo test --test make_baselines -- --ignored --nocapture
//! ```

use merge_to_master::prelude::*;

/// { dialogue id => { info id => [prev id, next id] } }, as integration_tests.rs reads it.
type DialogueData = std::collections::HashMap<String, std::collections::HashMap<String, [String; 2]>>;

const OPTIONS: MergeOptions = MergeOptions {
    remove_deleted: false,
    apply_moved_references: false,
    preserve_duplicate_references: false,
};

fn write_expect(case: &str, mut merged: PluginData) -> Result<()> {
    merged.remove_ignored();
    let mut data = DialogueData::new();
    for (id, group) in merged.dialogues {
        let infos = data.entry(id).or_default();
        for info in &group.infos {
            infos.insert(info.id.clone(), [info.prev_id.clone(), info.next_id.clone()]);
        }
    }
    let bytes = rkyv::to_bytes::<_, 4096>(&data).map_err(|e| anyhow::anyhow!("{e:?}"))?;
    let dir = format!("./ignore/{case}");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(format!("{dir}/Expect.rkyv"), &bytes)?;
    println!("{case}: {} topics written to {dir}/Expect.rkyv", data.len());
    Ok(())
}

#[test]
#[ignore = "writes ./ignore/*/Expect.rkyv - run it on upstream merge_to_master"]
fn make_dialogue_baselines() -> Result<()> {
    let mut written = 0;
    for case in ["MW", "MW_TB", "MW_BM", "MW_TB_BM"] {
        let master = PathBuf::from(format!("./ignore/{case}/Master.esm"));
        let plugin = PathBuf::from(format!("./ignore/{case}/Plugin.esp"));
        if !(master.is_file() && plugin.is_file()) {
            println!("{case}: no Master.esm / Plugin.esp - skipped");
            continue;
        }
        write_expect(case, merge_plugins(&plugin, &master, OPTIONS)?)?;
        written += 1;
    }

    let tb_bm = ["Morrowind.esm", "Tribunal.esm", "Bloodmoon.esm"].map(|f| PathBuf::from(format!("./ignore/TB_BM/{f}")));
    if tb_bm.iter().all(|p| p.is_file()) {
        if PathBuf::from("./ignore/MW_TB_BM/Expect.rkyv").is_file() {
            println!("TB_BM: checked against MW_TB_BM's file, already written");
        } else {
            let merged_path = PathBuf::from("./ignore/TB_BM/Merged.esp");
            merge_plugins(&tb_bm[2], &tb_bm[1], OPTIONS)?.save_path(&merged_path)?;
            write_expect("MW_TB_BM", merge_plugins(&merged_path, &tb_bm[0], OPTIONS)?)?;
            written += 1;
        }
    } else {
        println!("TB_BM: no Morrowind.esm / Tribunal.esm / Bloodmoon.esm - skipped");
    }

    assert!(written > 0, "nothing to record: put the cases' files under ./ignore/ first");
    Ok(())
}

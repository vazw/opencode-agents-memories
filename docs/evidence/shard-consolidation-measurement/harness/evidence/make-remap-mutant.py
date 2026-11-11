import shutil
from pathlib import Path

src = Path(".tmp/scratch/shard-measurement")
dst = Path(".tmp/scratch/shard-rework-mutant-remap")
if dst.exists():
    shutil.rmtree(dst)
(dst / "src").mkdir(parents=True)
(dst / "tests").mkdir()
shutil.copy(src / "Cargo.toml", dst / "Cargo.toml")
shutil.copy(src / "Cargo.lock", dst / "Cargo.lock")
shutil.copy(src / "src/lib.rs", dst / "src/lib.rs")
shutil.copy(src / "src/main.rs", dst / "src/main.rs")
shutil.copy(src / "tests/measurement.rs", dst / "tests/measurement.rs")
p = dst / "src/lib.rs"
t = p.read_text()
old = '''        db.execute(&format!("DELETE FROM {table}"), vec![])
            .await
            .map_err(engine)?;
        for row in &mapped {
            db.execute(
                &format!(
                    "INSERT INTO {table} ({}) VALUES ({})",
                    cols.join(","),
                    vec!["?"; cols.len()].join(",")
                ),
                row.clone(),
            )
            .await
            .map_err(engine)?;
        }
'''
assert t.count(old) == 1, t.count(old)
p.write_text(t.replace(old, ""))
print("remap mutant ready: auxiliary persistence block (DELETE + mapped INSERT) removed")

use super::*;
use std::io::{Seek, SeekFrom};

struct Source;

impl FileSource for Source {
    fn size(&self) -> u64 {
        3
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, CliError> {
        let bytes = &b"abc"[offset as usize..];
        let n = bytes.len().min(buf.len());
        buf[..n].copy_from_slice(&bytes[..n]);
        Ok(n)
    }
}

#[test]
fn corruption_after_sync_is_refused_before_publication() {
    // FIPS 180-2's published "abc" digest; the corrupted byte produces "xbc".
    const EXPECTED: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
    // Independently computed with `printf xbc | shasum -a 256`.
    const CORRUPTED: &str = "11f995635483d63d73611c514dba734e51e45a04ab45b7f29cbb1ee03127748f";
    for (tag, replace, len) in [("byte", true, 3), ("short", true, 2), ("long", false, 4)] {
        let dir =
            std::env::temp_dir().join(format!("rust-fs-core-verify-{}-{tag}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("out");
        if replace {
            fs::write(&dest, b"old").unwrap();
        }
        let err = export_to_file(&mut Source, &dest, replace, true, |file, _| {
            file.sync_all()?;
            if len == 3 {
                file.seek(SeekFrom::Start(0))?;
                file.write_all(b"x")?;
            } else {
                file.set_len(len)?;
            }
            file.sync_all()
        })
        .unwrap_err();
        assert_eq!(err.code, super::super::output::EXIT_FAILED);
        assert!(err.message.contains(EXPECTED), "{}", err.message);
        assert!(err.message.contains("sha256"), "{}", err.message);
        if len == 3 {
            assert!(err.message.contains(CORRUPTED), "{}", err.message);
        }
        if replace {
            assert_eq!(fs::read(&dest).unwrap(), b"old");
        } else {
            assert!(!dest.exists());
        }
        assert_eq!(fs::read_dir(&dir).unwrap().count(), usize::from(replace));
    }
}

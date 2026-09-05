//! Which disks are there, and which of them may be erased.
//!
//! `lsblk -P` rather than `lsblk -J`: the pairs form is `KEY="value"` per line,
//! which parses in twenty lines, and JSON would mean a serde dependency for one
//! command's output.
//!
//! The interesting part is not the listing but the **exclusions**. An installer
//! that offers you the USB stick it is running from will eventually be taken up
//! on it, so a disk holding a mounted filesystem is shown, marked, and not
//! selectable — shown rather than hidden, because a disk missing from the list
//! with no explanation is the other way to lose an afternoon.

use crate::run::Runner;

#[derive(Debug, Clone)]
pub struct Disk {
    pub path: String,
    pub bytes: u64,
    pub model: String,
    pub transport: String,
    pub removable: bool,
    /// `Some(reason)` if this disk must not be wiped.
    pub busy: Option<String>,
}

impl Disk {
    pub fn size(&self) -> String {
        human(self.bytes)
    }

    /// `/dev/sda` → `/dev/sda1`, `/dev/nvme0n1` → `/dev/nvme0n1p1`.
    ///
    /// The `p` is not cosmetic: kernel naming inserts it whenever the disk name
    /// already ends in a digit, and getting it wrong means formatting nothing
    /// and then mounting nothing, several steps later.
    pub fn partition(&self, n: u32) -> String {
        let digit = self.path.chars().last().is_some_and(|c| c.is_ascii_digit());
        if digit {
            format!("{}p{n}", self.path)
        } else {
            format!("{}{n}", self.path)
        }
    }

    /// What the confirmation screen asks the user to type.
    pub fn word(&self) -> String {
        self.path
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
            .to_string()
    }
}

/// Every whole disk on the machine, largest first.
pub fn disks(run: &mut Runner) -> Result<Vec<Disk>, String> {
    let listing = run
        .capture(&[
            "lsblk",
            "-dnb",
            "-o",
            "PATH,KNAME,SIZE,TYPE,RM,RO,MODEL,TRAN",
            "-P",
        ])
        .map_err(|e| format!("cannot list block devices: {e}"))?;
    // Every device, so a disk with something mounted anywhere in its tree can
    // be ruled out. **Not** `unwrap_or_default`: if this call fails, the
    // installer has no idea what is mounted, and the safe reading of "no idea"
    // is not "nothing". `-l` is deliberately absent — lsblk refuses
    // `--list --pairs`, and the version of this that passed both silently
    // produced an empty listing and offered the running system's disk.
    let tree = run
        .capture(&["lsblk", "-nb", "-o", "KNAME,PKNAME,MOUNTPOINT", "-P"])
        .map_err(|e| format!("cannot list mounted filesystems: {e}"))?;
    let nodes: Vec<Node> = tree
        .lines()
        .map(pairs)
        .map(|f| Node {
            kname: f.get("KNAME").cloned().unwrap_or_default(),
            parent: f.get("PKNAME").cloned().unwrap_or_default(),
            mountpoint: f.get("MOUNTPOINT").cloned().unwrap_or_default(),
        })
        .collect();

    let mut out: Vec<Disk> = Vec::new();
    for line in listing.lines() {
        let f = pairs(line);
        // `loop` as well as `disk`: an attached disk image is a legitimate
        // target and the only way to exercise this program without a spare
        // machine. A live ISO's own loop devices are all mounted, so the check
        // below removes them.
        if !matches!(
            f.get("TYPE").map(String::as_str),
            Some("disk") | Some("loop")
        ) {
            continue;
        }
        let path = f.get("PATH").cloned().unwrap_or_default();
        if path.is_empty() || path.starts_with("/dev/zram") {
            continue;
        }
        let bytes: u64 = f.get("SIZE").and_then(|s| s.parse().ok()).unwrap_or(0);
        let kname = f
            .get("KNAME")
            .cloned()
            .unwrap_or_else(|| path.rsplit('/').next().unwrap_or_default().to_string());
        let mut busy = None;
        if f.get("RO").map(String::as_str) == Some("1") {
            busy = Some("read-only".into());
        }
        // Anything under 4 GiB cannot hold an OSTree sysroot with three
        // generations in it, and is almost always the installation medium.
        if busy.is_none() && bytes < 4 << 30 {
            busy = Some("too small for a Kiln sysroot".into());
        }
        if busy.is_none() {
            busy = mounted_on(&nodes, &kname);
        }
        out.push(Disk {
            path,
            bytes,
            model: f.get("MODEL").cloned().unwrap_or_default(),
            transport: f.get("TRAN").cloned().unwrap_or_default(),
            removable: f.get("RM").map(String::as_str) == Some("1"),
            busy,
        });
    }
    out.sort_by_key(|d| std::cmp::Reverse(d.bytes));
    Ok(out)
}

/// One row of `lsblk`'s device tree.
struct Node {
    kname: String,
    parent: String,
    mountpoint: String,
}

/// The reason a disk is off limits, if it has one.
///
/// The **chain** is the point. A partition mounted straight off the disk is the
/// easy case; the one that matters is `dm-0` mounted at `/`, whose parent is
/// `nvme0n1p2`, whose parent is the disk. Checking only direct children offers
/// you the LUKS or LVM disk you are running from, and it looks correct right up
/// until somebody with an encrypted root uses it.
fn mounted_on(nodes: &[Node], disk: &str) -> Option<String> {
    let parent_of = |k: &str| {
        nodes
            .iter()
            .find(|n| n.kname == k)
            .map(|n| n.parent.clone())
            .filter(|p| !p.is_empty())
    };
    for node in nodes.iter().filter(|n| !n.mountpoint.is_empty()) {
        let mut at = node.kname.clone();
        // Depth-capped: a cycle in what the kernel reports is not something to
        // hang the installer on.
        for _ in 0..8 {
            if at == disk {
                let what = if node.mountpoint == "[SWAP]" {
                    "in use — swap is active on it".to_string()
                } else {
                    format!("in use — {} is mounted from it", node.mountpoint)
                };
                return Some(what);
            }
            match parent_of(&at) {
                Some(p) => at = p,
                None => break,
            }
        }
    }
    None
}

/// `KEY="value" KEY="value"` → a map. lsblk escapes an embedded quote as `\x22`,
/// which nothing here needs to decode: a model name containing a quote loses a
/// character in a label, and that is the whole consequence.
fn pairs(line: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let mut rest = line.trim();
    while let Some(eq) = rest.find("=\"") {
        let key = rest[..eq].trim().to_string();
        let after = &rest[eq + 2..];
        let Some(end) = after.find('"') else { break };
        out.insert(key, after[..end].trim().to_string());
        rest = &after[end + 1..];
    }
    out
}

/// Sizes the way a disk's box prints them: powers of two, one decimal.
pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u + 1 < UNITS.len() {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1}{}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_names_follow_the_kernel() {
        let sd = Disk {
            path: "/dev/sda".into(),
            bytes: 0,
            model: String::new(),
            transport: String::new(),
            removable: false,
            busy: None,
        };
        assert_eq!(sd.partition(1), "/dev/sda1");
        let nvme = Disk {
            path: "/dev/nvme0n1".into(),
            ..sd
        };
        assert_eq!(nvme.partition(3), "/dev/nvme0n1p3");
    }

    #[test]
    fn pairs_parses_lsblk() {
        let m = pairs(r#"PATH="/dev/sda" SIZE="500107862016" TYPE="disk" MODEL="CT500MX500SSD1""#);
        assert_eq!(m["PATH"], "/dev/sda");
        assert_eq!(m["SIZE"], "500107862016");
        assert_eq!(m["MODEL"], "CT500MX500SSD1");
    }

    fn node(kname: &str, parent: &str, mountpoint: &str) -> Node {
        Node {
            kname: kname.into(),
            parent: parent.into(),
            mountpoint: mountpoint.into(),
        }
    }

    #[test]
    fn an_empty_mountpoint_is_not_a_reason() {
        let idle = [node("sda", "", ""), node("sda1", "sda", "")];
        assert!(mounted_on(&idle, "sda").is_none());
        let live = [
            node("sda", "", ""),
            node("sda1", "sda", "/run/archiso/bootmnt"),
        ];
        assert!(mounted_on(&live, "sda").is_some());
    }

    /// The case a direct-children check gets wrong: an encrypted or LVM root.
    #[test]
    fn a_mount_two_levels_down_still_rules_the_disk_out() {
        let luks = [
            node("nvme0n1", "", ""),
            node("nvme0n1p2", "nvme0n1", ""),
            node("dm-0", "nvme0n1p2", "/"),
        ];
        let why = mounted_on(&luks, "nvme0n1").expect("the running root rules its disk out");
        assert!(why.contains('/'), "{why}");
        // …and says nothing about a disk that is genuinely free.
        assert!(mounted_on(&luks, "sdb").is_none());
    }

    #[test]
    fn active_swap_counts() {
        let swapping = [node("sda", "", ""), node("sda2", "sda", "[SWAP]")];
        assert!(mounted_on(&swapping, "sda").unwrap().contains("swap"));
    }

    /// A parent chain that loops must not hang the installer.
    #[test]
    fn a_cycle_terminates() {
        let bad = [node("a", "b", "/"), node("b", "a", "")];
        assert!(mounted_on(&bad, "zz").is_none());
    }
}

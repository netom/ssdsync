use {
    clap::Parser,
    indicatif::{
        ProgressBar,
        ProgressStyle,
    },
    nix::ioctl_read,
    std::{
        io::SeekFrom,
        os::unix::{
            fs::FileTypeExt,
            io::AsRawFd,
        },
    },
    tokio::{
        fs::{
            File,
            OpenOptions,
        },
        io::{
            AsyncReadExt,
            AsyncWriteExt,
            AsyncSeekExt,
        },
        join,
        sync::mpsc,
    },
};

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Source file or device
    source: String,

    /// Target file or device
    target: String,

    /// Size of blocks in bytes to read/write at once
    #[clap(short, long, default_value_t = 4096 * 8)]
    block_size: usize,
}

#[derive(PartialEq, Eq, Clone)]
struct Buf {
    length: usize,
    data: Vec<u8>
}

impl Buf {
    fn as_slice(&self) -> &[u8] {
        let (ret, _) = self.data.as_slice().split_at(self.length);
        ret
    }
}

// See linux/fs.h
const BLKGETSIZE64_CODE: u8 = 0x12;
const BLKGETSIZE64_SEQ: u8 = 114;

ioctl_read!(ioctl_blkgetsize64, BLKGETSIZE64_CODE, BLKGETSIZE64_SEQ, u64);

async fn get_size(f: &File) -> u64 {
    let meta = f.metadata().await.unwrap();
    let file_type = meta.file_type();

    if file_type.is_file() {
        meta.len()
    } else if file_type.is_block_device() {
        let mut size: u64 = 0;
        let size_ptr = &mut size as *mut u64;
        let std_file = f.try_clone().await.unwrap().into_std().await;
        unsafe {
            ioctl_blkgetsize64(std_file.as_raw_fd(), size_ptr).unwrap();
        }
        size
    } else {
        panic!("Only regular files, block devices and symlinks to them are supported.");
    }
}

async fn read_block(
    mut buf: Buf,
    file: &mut File,
    buf_fw_tx: &tokio::sync::mpsc::Sender<Buf>,
) -> bool {
    buf.length = match file.read(&mut buf.data).await {
        Err(e) => panic!("{}", e),
        Ok(n) => n
    };
    if buf.length == 0 {
        // No more to read
        return false;
    }
    if let Err(_) = buf_fw_tx.send(buf).await {
        // No one to receive
        return false;
    }
    return true;
}

async fn read_blocks(
    block_size: usize,
    num_bufs: usize,
    mut file: File,
    buf_fw_tx: tokio::sync::mpsc::Sender<Buf>,
    mut buf_bk_rx: tokio::sync::mpsc::Receiver<Buf>,
) {
    for _ in 0..num_bufs {
        let buf = Buf {
            length: 0,
            data: vec![0_u8; block_size]
        };
        if !read_block(buf, &mut file, &buf_fw_tx).await {
            return;
        };
    }
    while let Some(buf) = buf_bk_rx.recv().await {
        if !read_block(buf, &mut file, &buf_fw_tx).await {
            return;
        };
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = Args::parse();

    let source_name = &args.source;
    let target_name = &args.target;

    // Read both file sizes
    let source_r = File::open(source_name).await.unwrap();
    let target_r = File::open(target_name).await.unwrap();
    let mut target_w = OpenOptions::new().write(true).open(target_name).await.unwrap();

    let source_size = get_size(&source_r).await;
    let target_size = get_size(&target_r).await;

    println!("{} -> {}", source_size, target_size);

    //(source_size == target_size).ok_or("Lengths should match").unwrap();

    let bar = ProgressBar::new(source_size);

    bar.set_style(ProgressStyle::default_bar()
        .template("{wide_bar} [{percent:>3}% {bytes_per_sec} ETA: {eta_precise}]")
        .progress_chars("##-"));

    let block_size = 4096 * 8;
    let num_bufs = 16;

    let (src_fw_tx, mut src_fw_rx) = mpsc::channel(num_bufs);
    let (src_bk_tx, src_bk_rx) = mpsc::channel(num_bufs);
    let (tgt_fw_tx, mut tgt_fw_rx) = mpsc::channel(num_bufs);
    let (tgt_bk_tx, tgt_bk_rx) = mpsc::channel(num_bufs);

    // Reads source
    tokio::spawn(read_blocks(
        block_size, num_bufs, source_r,
        src_fw_tx, src_bk_rx
    ));

    // Reads target
    tokio::spawn(read_blocks(
        block_size, num_bufs, target_r,
        tgt_fw_tx, tgt_bk_rx
    ));

    let mut total = 0;
    let mut diff = 0;
    let mut pos = 0;

    // Compares buffers and writes target
    while let (Some(src), Some(tgt)) = join!(src_fw_rx.recv(), tgt_fw_rx.recv()) {
        if src.length == 0 || tgt.length == 0 || src.length != tgt.length {
            println!("Done.");
            break;
        }

        let n = src.length;

        bar.inc(n as u64);

        if src != tgt {
            if let Err(e) = target_w.seek(SeekFrom::Start(pos)).await {
                println!("Failed to seek, exiting: {}", e);
                break;
            }
            if let Err(e) = target_w.write(&src.as_slice()).await {
                println!("Failed to write, exiting: {}", e);
                break;
            }
            diff += 1;
        }

        let _ = join!(src_bk_tx.send(src), tgt_bk_tx.send(tgt));

        total += 1;
        pos += n as u64;
    }

    bar.finish_at_current_pos();

    println!("\nTotal: {}, different: {}", total, diff);
}

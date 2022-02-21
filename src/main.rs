use {
    //boolinator::Boolinator,
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

#[tokio::main]
async fn main() {
    let args = Args::parse();

    let source_name = &args.source;
    let target_name = &args.target;

    // Read both file sizes
    let mut source_r = File::open(source_name).await.unwrap();
    let mut target_r = File::open(target_name).await.unwrap();
    let mut target_w = OpenOptions::new().write(true).open(target_name).await.unwrap();

    //let source_meta = source.metadata().await.unwrap();
    //let target_meta = target.metadata().await.unwrap();

    let source_size = get_size(&source_r).await;
    let target_size = get_size(&target_r).await;

    println!("{} -> {}", source_size, target_size);

    //(source_size == target_size).ok_or("Lengths should match").unwrap();

    let bar = ProgressBar::new(source_size);

    bar.set_style(ProgressStyle::default_bar()
        .template("{wide_bar} [{percent:>3}% {bytes_per_sec} ETA: {eta_precise}]")
        .progress_chars("##-"));

    let block_size = 4096 * 8;

    let (src_tx, mut src_rx) = mpsc::channel(10);
    let (tgt_tx, mut tgt_rx) = mpsc::channel(10);

    // Reads source
    tokio::spawn( async move {
        let mut buf = vec![0_u8; block_size];
        loop {
            let n = match source_r.read(&mut buf).await {
                Err(e) => panic!("{}", e),
                Ok(n) => n
            };
            if n == 0 {
                // No more to read
                return;
            }
            if let Err(_) = src_tx.send(buf[0..n].to_vec()).await {
                // No one to receive
                return;
            }
        }
    });

    // Reads target
    tokio::spawn( async move {
        let mut buf = vec![0_u8; block_size];
        loop {
            let n = match target_r.read(&mut buf).await {
                Err(e) => panic!("{}", e),
                Ok(n) => n
            };
            if n == 0 {
                // No more to read
                return;
            }
            if let Err(_) = tgt_tx.send(buf[0..n].to_vec()).await {
                // No one to receive
                return;
            }
        }
    });

    let mut total = 0;
    let mut diff = 0;
    let mut pos = 0;

    // Compares buffers and writes target
    while let (Some(src), Some(tgt)) = join!(src_rx.recv(), tgt_rx.recv()) {
        let src_l = src.len();
        let tgt_l = tgt.len();

        if src_l == 0 || tgt_l == 0 || src_l != tgt_l {
            println!("Done.");
            break;
        }

        let n = src_l;

        bar.inc(n as u64);

        if src != tgt {
            //println!("Difference at position {}", pos);
            if let Err(e) = target_w.seek(SeekFrom::Start(pos)).await {
                println!("Failed to seek, exiting: {}", e);
                break;
            }
            if let Err(e) = target_w.write(&src).await {
                println!("Failed to write, exiting: {}", e);
                break;
            }
            diff += 1;
        }
        total += 1;
        pos += n as u64;
    }

    bar.finish_at_current_pos();

    println!("\nTotal: {}, different: {}", total, diff);
}

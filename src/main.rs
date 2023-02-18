use std::collections::VecDeque;

use {
    clap::Parser,
    indicatif::{ProgressBar, ProgressStyle},
    nix::ioctl_read,
    std::{
        io::SeekFrom,
        os::unix::{fs::FileTypeExt, io::AsRawFd},
    },
    tokio::{
        fs::{File, OpenOptions},
        io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
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
    data: Vec<u8>,
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

async fn read_blocks(
    mut file: File,
    mut buf_rx: tokio::sync::mpsc::Receiver<Buf>,
    buf_tx: tokio::sync::mpsc::Sender<Buf>,
) {
    while let Some(mut buf) = buf_rx.recv().await {
        buf.length = match file.read(&mut buf.data).await {
            Err(e) => panic!("{}", e),
            Ok(n) => n,
        };
        if let Err(_) = buf_tx.send(buf).await {
            // Nobody's listening
            return;
        }
    }
}

async fn write_blocks(
    mut f: File,
    mut buf_rx: tokio::sync::mpsc::Receiver<(u64, Buf)>,
    buf_tx: tokio::sync::mpsc::Sender<Buf>,
) {
    while let Some((pos, buf)) = buf_rx.recv().await {
        // TODO: be smart about seek. Call only when needed.
        if let Err(e) = f.seek(SeekFrom::Start(pos)).await {
            println!("Failed to seek, exiting: {}", e);
            return;
        }
        match f.write(&buf.as_slice()).await {
            Ok(written) => {
                if written != buf.length {
                    println!(
                        "Could not write {} bytes, only {}, exiting.",
                        buf.length, written
                    );
                    return;
                }
            }
            Err(e) => {
                println!("Failed to write, exiting: {}", e);
                return;
            }
        }

        if let Err(_) = buf_tx.send(buf).await {
            // Nobody's listening
            return;
        }
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
    let target_w = OpenOptions::new()
        .write(true)
        .open(target_name)
        .await
        .unwrap();

    let source_size = get_size(&source_r).await;
    let target_size = get_size(&target_r).await;

    println!("{} -> {}", source_size, target_size);

    //(source_size == target_size).ok_or("Lengths should match").unwrap();

    let bar = ProgressBar::new(source_size);

    bar.set_style(
        ProgressStyle::default_bar()
            .template("{wide_bar} [{percent:>3}% {bytes_per_sec} ETA: {eta_precise}]")
            .expect("Template error")
            .progress_chars("##-"),
    );

    let block_size = 16 * 1024;
    let num_bufs = 16;

    // Channels for talking with the source file reader task
    let (src_fw_tx, src_fw_rx) = mpsc::channel(num_bufs);
    let (src_bk_tx, mut src_bk_rx) = mpsc::channel(num_bufs);

    // Channels for talking with the target file reader task
    let (tgt_r_fw_tx, tgt_r_fw_rx) = mpsc::channel(num_bufs);
    let (tgt_r_bk_tx, mut tgt_r_bk_rx) = mpsc::channel(num_bufs);

    // Channels for talking with the target file writer task
    let (tgt_w_fw_tx, tgt_w_fw_rx) = mpsc::channel(num_bufs);
    let (tgt_w_bk_tx, mut tgt_w_bk_rx) = mpsc::channel(num_bufs);

    // Reads source
    tokio::spawn(read_blocks(source_r, src_fw_rx, src_bk_tx));

    // Reads target
    tokio::spawn(read_blocks(target_r, tgt_r_fw_rx, tgt_r_bk_tx));

    // Writes target
    tokio::spawn(write_blocks(target_w, tgt_w_fw_rx, tgt_w_bk_tx));

    let mut total = 0;
    let mut diff = 0;
    let mut pos = 0;

    // Allocate a pool of buffers, l=4
    let mut buffers: VecDeque<Buf> = VecDeque::with_capacity(4);
    for _ in 0..4 {
        buffers.push_back(Buf {
            data: vec![0; block_size],
            length: 0,
        });
    }

    // Send the first pair of buffers to the readers
    // Wait for them to be sent back
    let mut bsrc: Buf;
    let mut btgt: Buf;
    let (_, _, bsrc_, btgt_) = join!(
        src_fw_tx.send(buffers.pop_front().unwrap()),
        tgt_r_fw_tx.send(buffers.pop_front().unwrap()),
        src_bk_rx.recv(),
        tgt_r_bk_rx.recv()
    );
    bsrc = bsrc_.unwrap(); // TODO: handle dropped tx?
    btgt = btgt_.unwrap(); // TODO: handle dropped tx?

    loop {
        // We have a pair of buffers from the readers

        // Check wether we're done
        if bsrc.length == 0 || btgt.length == 0 || bsrc.length != btgt.length {
            println!("Done.");
            break;
        }

        let n = bsrc.length;

        bar.inc(n as u64);

        // Send a couple of new buffers back from the pool
        let (_, _) = join!(
            src_fw_tx.send(buffers.pop_front().unwrap()),
            tgt_r_fw_tx.send(buffers.pop_front().unwrap())
        ); // TODO: handle unsuccessful send?

        // Compare the arrived buffers
        // If they match:
        //   Return the buffers to the pool
        //   Wait for buffers from the readers
        //   Start from the beginning
        if bsrc == btgt {
            buffers.push_back(bsrc);
            buffers.push_back(btgt);
            let (bsrc_, btgt_) = join!(src_bk_rx.recv(), tgt_r_bk_rx.recv());
            bsrc = bsrc_.unwrap(); // TODO: handle dropped tx?
            btgt = btgt_.unwrap(); // TODO: handle dropped tx?
            continue;
        }
        // They're different.

        // Return the one from the target reader to the pool
        buffers.push_back(btgt);

        // Send the one arrived from the source reader to the writer
        // Wait for buffers from the writer and the readers
        let (_, bw, bsrc_, btgt_) = join!(
            tgt_w_fw_tx.send((pos, bsrc)),
            tgt_w_bk_rx.recv(),
            src_bk_rx.recv(),
            tgt_r_bk_rx.recv()
        );
        bsrc = bsrc_.unwrap(); // TODO: handle dropped tx?
        btgt = btgt_.unwrap(); // TODO: handle dropped tx?

        // Return the writer's buffer to the pool
        buffers.push_back(bw.unwrap()); // TODO: handle dropped tx?

        diff += 1;
        total += 1;
        pos += n as u64;
    }

    bar.finish();

    println!("\nTotal: {}, different: {}", total, diff);
}

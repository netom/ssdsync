use {
    boolinator::Boolinator,
    clap::{
        Arg,
        App
    },
    indicatif::{
        ProgressBar,
        ProgressStyle,
    },
    nix::ioctl_read,
    std::os::unix::{
        fs::FileTypeExt,
        io::AsRawFd,
    },
    tokio::{
        fs::File,
        io::AsyncReadExt,
    },
};

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
    let matches = App::new("SSD Sync")
        .version("1.0")
        .author("Tamas Fabian <giganetom@gmail.com>")
        .about("Copy data between SSDs keeping doing as few writes as possible.")
        .arg(Arg::with_name("source")
            .short("s")
            .long("source")
            .value_name("SOURCE")
            .help("copy data from here")
            .takes_value(true)
            .required(true)
            .index(1))
        .arg(Arg::with_name("target")
            .short("t")
            .long("target")
            .value_name("TARGET")
            .help("write here")
            .takes_value(true)
            .required(true)
            .index(2))
        .get_matches();

    let source_name = matches.value_of("source").ok_or("Source name is mandatory").unwrap();
    let target_name = matches.value_of("target").ok_or("Target name is mandatory").unwrap();

    // Read both file sizes
    let mut source = File::open(source_name).await.unwrap();
    let target = File::open(target_name).await.unwrap();

    //let source_meta = source.metadata().await.unwrap();
    //let target_meta = target.metadata().await.unwrap();

    let source_size = get_size(&source).await;
    let target_size = get_size(&target).await;

    println!("{} -> {}", source_size, target_size);

    //(source_size == target_size).ok_or("Lengths should match").unwrap();

    let bar = ProgressBar::new(source_size);

    bar.set_style(ProgressStyle::default_bar()
        .template("{wide_bar} [{percent:>3}% ETA: {eta_precise}, {elapsed_precise} / {duration_precise}]")
        .progress_chars("##-"));

    let block_size = 409600;

    let mut buffer: Vec<u8> = vec![0; block_size];

    loop {
        let n = source.read(&mut buffer).await.unwrap();
        if n == 0 {
            break;
        }
        bar.inc(n as u64);
    }

    // Determine the number of blocks to read

    // Start loop. In the loop

    // Read two N-blocks in parallel

    // Determine the blocks to write

    // Write differing blocks

    // Repeat

}

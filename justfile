default:
  cargo build --release

uf2:
  elf2uf2-rs target/thumbv7em-none-eabihf/release/corgie-board target/firmware.uf2

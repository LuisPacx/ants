![screenshot](/assets/logo.jpg)

# Ant Colony Simulation
This is an ant colony simulation, it internally uses kdtree and query caching, it's able to handle about 5k solana wallets on the cpu.


Built with [Rust](https://www.rust-lang.org/) and [Bevy](https://bevyengine.org/) game engine

![screenshot](/screenshot.png)

## Configurations
- The project config file is located at `src/configs.rs`
- If all ants aren't forming a single trail even after a long time, try increasing `ANT_INITIAL_PH_STRENGTH` in the configs to a greater value (exmaple: `40.0`)
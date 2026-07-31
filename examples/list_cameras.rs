//! Print the cameras each compiled-in backend can see. Diagnostic for bring-up:
//! `cargo run --release --example list_cameras`.

fn main() {
    #[cfg(numanager_backend)]
    {
        println!("-- nu-manager devices --");
        match opengel::camera::numanager_backend::list_cameras() {
            Ok(cams) if cams.is_empty() => println!("  (none found)"),
            Ok(cams) => {
                for cam in cams {
                    println!("  [{}] {}", cam.index, cam.name);
                }
            }
            Err(e) => println!("  discovery failed: {e}"),
        }
    }

    #[cfg(nokhwa_backend)]
    {
        println!("-- webcams (nokhwa) --");
        match opengel::camera::nokhwa_backend::list_cameras() {
            Ok(cams) if cams.is_empty() => println!("  (none found)"),
            Ok(cams) => {
                for cam in cams {
                    println!("  [{}] {}", cam.index, cam.name);
                }
            }
            Err(e) => println!("  query failed: {e}"),
        }
    }
}

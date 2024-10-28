//! App related code
//!
//! Directory structure:
//! ```text
//! .teepod/
//! ├── image
//! │   └── ubuntu-24.04
//! │       ├── hda.img
//! │       ├── info.json
//! │       ├── initrd.img
//! │       ├── kernel
//! │       └── rootfs.iso
//! └── vm
//!     └── e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
//!         └── shared
//!             └── docker-compose.yaml
//! ```
use crate::config::Config;
use crate::vm::run::{Image, TdxConfig, VmConfig, VmMonitor};

use anyhow::{bail, Context, Result};
use bon::Builder;
use fs_err as fs;
use id_pool::IdPool;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use teepod_rpc::VmInfo;

mod id_pool;

#[derive(Deserialize, Serialize, Builder)]
pub struct Manifest {
    id: String,
    name: String,
    app_id: String,
    vcpu: u32,
    memory: u32,
    disk_size: u32,
    image: String,
    port_map: HashMap<u16, u16>,
}

#[derive(Clone)]
pub struct App {
    pub config: Arc<Config>,
    state: Arc<Mutex<AppState>>,
}

pub(crate) struct AppState {
    monitor: VmMonitor,
    cid_pool: IdPool<u32>,
}

impl App {
    pub(crate) fn vm_dir(&self) -> PathBuf {
        self.config.run_path.clone().into()
    }

    pub fn new(config: Config) -> Self {
        let cid_start = config.cvm.cid_start;
        let cid_end = cid_start.saturating_add(config.cvm.cid_pool_size);
        let cid_pool = IdPool::new(cid_start, cid_end);
        Self {
            state: Arc::new(Mutex::new(AppState {
                cid_pool,
                monitor: VmMonitor::new(config.qemu_path.clone()),
            })),
            config: Arc::new(config),
        }
    }

    pub fn load_vm(&self, work_dir: impl AsRef<Path>) -> Result<()> {
        let manifest_path = work_dir.as_ref().join("vm-manifest.json");
        let manifest = fs::read_to_string(manifest_path).context("Failed to read manifest")?;
        let manifest: Manifest =
            serde_json::from_str(&manifest).context("Failed to parse manifest")?;
        let todo = "sanitize the image name";
        let image_path = self.config.image_path.join(&manifest.image);
        let image = Image::load(&image_path).context("Failed to load image")?;

        let cid = self
            .state
            .lock()
            .unwrap()
            .cid_pool
            .allocate()
            .context("CID pool exhausted")?;

        let vm_config = VmConfig {
            id: manifest.id,
            app_id: manifest.app_id,
            name: manifest.name,
            vcpu: manifest.vcpu,
            memory: manifest.memory,
            image,
            tdx_config: Some(TdxConfig { cid }),
            port_map: Default::default(),
            disk_size: manifest.disk_size,
        };
        if vm_config.disk_size > self.config.cvm.max_disk_size {
            bail!(
                "disk size too large, max size is {}",
                self.config.cvm.max_disk_size
            );
        }
        let result = self
            .state
            .lock()
            .unwrap()
            .monitor
            .run_vm(vm_config, work_dir.as_ref());
        if let Err(err) = result {
            println!("Failed to run VM: {err}");
        }
        Ok(())
    }

    pub fn stop_vm(&self, id: &str) -> Result<()> {
        self.state.lock().unwrap().monitor.stop_vm(id)?;
        Ok(())
    }

    pub fn reload_vms(&self) -> Result<()> {
        let vm_path = self.vm_dir();
        if vm_path.exists() {
            for entry in fs::read_dir(vm_path).context("Failed to read VM directory")? {
                let entry = entry.context("Failed to read directory entry")?;
                let vm_path = entry.path();
                if vm_path.is_dir() {
                    if let Err(err) = self.load_vm(vm_path) {
                        println!("Failed to load VM: {err}");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn list_vms(&self) -> Vec<VmInfo> {
        let mut infos = self
            .state
            .lock()
            .unwrap()
            .monitor
            .iter_vms()
            .map(|vm| vm.info())
            .collect::<Vec<_>>();

        infos.sort_by(|a, b| b.uptime_ms.cmp(&a.uptime_ms));
        let gw = &self.config.gateway;

        infos
            .into_iter()
            .map(|info| VmInfo {
                id: info.id,
                name: info.name,
                status: info.status.to_string(),
                uptime: info.uptime,
                app_url: format!("https://{}.{}:{}", info.app_id, gw.base_domain, gw.port),
                app_id: info.app_id,
            })
            .collect()
    }

    pub fn get_log_file(&self, id: &str) -> Result<PathBuf> {
        self.state.lock().unwrap().monitor.get_log_file(id)
    }

    pub fn list_image_names(&self) -> Result<Vec<String>> {
        let image_path = self.config.image_path.clone();
        let images = fs::read_dir(image_path).context("Failed to read image directory")?;
        Ok(images
            .flat_map(|e| {
                Some(
                    e.ok()?
                        .path()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .to_string(),
                )
            })
            .collect())
    }
}

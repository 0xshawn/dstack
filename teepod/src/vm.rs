pub(crate) mod image {
    use fs_err as fs;
    use std::path::{Path, PathBuf};

    use anyhow::{bail, Context, Result};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize)]
    pub struct ImageInfo {
        pub cmdline: Option<String>,
        pub kernel: String,
        pub initrd: String,
        pub hda: Option<String>,
        pub rootfs: Option<String>,
        pub bios: Option<String>,
        pub rootfs_hash: Option<String>,
    }

    impl ImageInfo {
        pub fn load(filename: PathBuf) -> Result<Self> {
            let file = fs::File::open(filename).context("failed to open image info")?;
            let info: ImageInfo =
                serde_json::from_reader(file).context("failed to parse image info")?;
            Ok(info)
        }
    }

    #[derive(Debug)]
    pub struct Image {
        pub info: ImageInfo,
        pub initrd: PathBuf,
        pub kernel: PathBuf,
        pub hda: Option<PathBuf>,
        pub rootfs: Option<PathBuf>,
        pub bios: Option<PathBuf>,
    }

    impl Image {
        pub fn load(base_path: impl AsRef<Path>) -> Result<Self> {
            let base_path = fs::canonicalize(base_path.as_ref())?;
            let info = ImageInfo::load(base_path.join("metadata.json"))?;
            let initrd = base_path.join(&info.initrd);
            let kernel = base_path.join(&info.kernel);
            let hda = info.hda.as_ref().map(|hda| base_path.join(hda));
            let rootfs = info.rootfs.as_ref().map(|rootfs| base_path.join(rootfs));
            let bios = info.bios.as_ref().map(|bios| base_path.join(bios));
            Self {
                info,
                hda,
                initrd,
                kernel,
                rootfs,
                bios,
            }
            .ensure_exists()
        }

        fn ensure_exists(self) -> Result<Self> {
            if !self.initrd.exists() {
                bail!("Initrd does not exist: {}", self.initrd.display());
            }
            if !self.kernel.exists() {
                bail!("Kernel does not exist: {}", self.kernel.display());
            }
            if let Some(hda) = &self.hda {
                if !hda.exists() {
                    bail!("Hda does not exist: {}", hda.display());
                }
            }
            if let Some(rootfs) = &self.rootfs {
                if !rootfs.exists() {
                    bail!("Rootfs does not exist: {}", rootfs.display());
                }
            }
            if let Some(bios) = &self.bios {
                if !bios.exists() {
                    bail!("Bios does not exist: {}", bios.display());
                }
            }
            Ok(self)
        }
    }
}

pub(crate) mod run {
    use crate::app::Manifest;

    pub use super::image::Image;
    pub use super::qemu::{TdxConfig, VmConfig};
    use anyhow::{bail, Context, Result};
    use fs_err as fs;
    use serde::Deserialize;
    use shared_child::SharedChild;
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tracing::{error, info};

    #[derive(Debug, Deserialize)]
    pub struct InstanceInfo {
        pub app_id: String,
        pub instance_id: String,
    }

    pub struct VmInstance {
        config: VmConfig,
        workdir: PathBuf,
        process: Option<Arc<SharedChild>>,
        started: bool,
        exited_at: Option<Instant>,
        started_at: Option<Instant>,
    }

    impl VmInstance {
        pub fn new(config: VmConfig, workdir: PathBuf) -> Self {
            Self {
                config,
                workdir,
                process: None,
                started: false,
                exited_at: None,
                started_at: None,
            }
        }

        pub fn instance_info(&self) -> Result<InstanceInfo> {
            let info_file = self.workdir.join("shared").join(".instance_info");
            let info: InstanceInfo = serde_json::from_slice(&fs::read(&info_file)?)?;
            Ok(info)
        }

        pub fn start(&mut self, qemu_bin: &Path) -> Result<()> {
            if self.is_running() {
                bail!("The instance is already running");
            }
            let process = super::qemu::run_vm(qemu_bin, &self.config, &self.workdir)?;
            let cloned_child = process.clone();
            let vmid = self.config.manifest.id.clone();
            std::thread::spawn(move || {
                let span = tracing::info_span!("wait_on_child", id = vmid);
                let _enter = span.enter();
                let status = match cloned_child.wait() {
                    Ok(status) => status,
                    Err(e) => {
                        error!("Failed to wait on child: {e:?}");
                        return;
                    }
                };
                if status.success() {
                    info!("The instance exited successfully");
                } else {
                    let todo = "Dont show error if VM is stopped by user";
                    error!("The instance exited with status: {:#?}", status);
                    if let Some(mut output) = cloned_child.take_stderr() {
                        let mut stderr = String::new();
                        match output.read_to_string(&mut stderr) {
                            Ok(_) => {
                                if !stderr.is_empty() {
                                    error!("The instance stderr: {:#?}", stderr);
                                }
                            }
                            Err(e) => error!("Failed to read the instance stderr: {e:?}"),
                        }
                    }
                    if let Some(mut output) = cloned_child.take_stdout() {
                        let mut stdout = String::new();
                        match output.read_to_string(&mut stdout) {
                            Ok(_) => {
                                if !stdout.is_empty() {
                                    info!("The instance stdout: {:#?}", stdout);
                                }
                            }
                            Err(e) => error!("Failed to read the instance stdout: {e:?}"),
                        }
                    }
                }
            });
            self.process = Some(process);
            self.started_at = Some(Instant::now());
            self.started = true;
            Ok(())
        }

        pub fn stop(&mut self) -> Result<()> {
            self.started = false;
            if let Some(process) = &self.process {
                process.kill()?;
                self.exited_at = Some(Instant::now());
            }
            Ok(())
        }

        pub fn is_running(&self) -> bool {
            match &self.process {
                Some(child) => match child.try_wait() {
                    Ok(None) => true,
                    _ => false,
                },
                None => false,
            }
        }

        pub fn info(&self) -> VmInfo {
            fn truncate(d: Duration) -> Duration {
                Duration::from_secs(d.as_secs())
            }
            let uptime = self.started_at.map(|t| t.elapsed());
            let uptime = uptime
                .map(|d| humantime::format_duration(truncate(d)).to_string())
                .unwrap_or_default();
            let status = match (self.started, self.is_running()) {
                (true, true) => "running",
                (true, false) => "exited",
                (false, true) => "stopping",
                (false, false) => "stopped",
            };
            let instance_id = self.instance_info().ok().map(|info| info.instance_id);
            VmInfo {
                manifest: self.config.manifest.clone(),
                workdir: self.workdir.clone(),
                instance_id,
                status,
                uptime,
                exited_at: None,
            }
        }
    }

    pub struct VmInfo {
        pub manifest: Manifest,
        pub workdir: PathBuf,
        pub status: &'static str,
        pub uptime: String,
        pub exited_at: Option<String>,
        pub instance_id: Option<String>,
    }

    pub struct VmMonitor {
        qemu_bin: PathBuf,
        vms: BTreeMap<String, VmInstance>,
    }

    impl VmMonitor {
        pub fn new(qemu_bin: PathBuf) -> Self {
            Self {
                qemu_bin,
                vms: BTreeMap::new(),
            }
        }

        pub fn load_vm(
            &mut self,
            vm: VmConfig,
            workdir: impl AsRef<Path>,
            start: bool,
        ) -> Result<()> {
            let mut vm = VmInstance::new(vm, workdir.as_ref().to_path_buf());
            if start {
                vm.start(&self.qemu_bin)?;
            }
            self.vms.insert(vm.config.manifest.id.clone(), vm);
            Ok(())
        }

        pub fn start_vm(&mut self, id: &str) -> Result<()> {
            let Some(vm) = self.vms.get_mut(id) else {
                bail!("The instance {} is not found", id);
            };
            vm.start(&self.qemu_bin)?;
            Ok(())
        }

        pub fn stop_vm(&mut self, id: &str) -> Result<()> {
            let Some(vm) = self.vms.get_mut(id) else {
                bail!("The instance {} is not found", id);
            };
            vm.stop()?;
            Ok(())
        }

        pub fn remove_vm(&mut self, id: &str) -> Result<()> {
            {
                let vm = self.vms.get(id).context("VM not found")?;
                if vm.is_running() || vm.started {
                    bail!("The instance is running, please stop it first");
                }
            }
            self.vms.remove(id);
            Ok(())
        }

        pub fn get_log_file(&self, id: &str) -> Result<PathBuf> {
            let Some(info) = self.vms.get(id) else {
                bail!("The instance {} is not found", id);
            };
            super::qemu::get_log_file(&info.workdir).context("Failed to get log")
        }

        pub fn iter_vms(&self) -> impl Iterator<Item = &VmInstance> {
            self.vms.values()
        }
    }
}

mod qemu {
    //! QEMU related code
    use std::{
        path::{Path, PathBuf},
        process::Command,
        sync::Arc,
    };

    use crate::{app::Manifest, config::Networking};

    use super::image::Image;
    use anyhow::Result;
    use bon::Builder;
    use fs_err as fs;
    use shared_child::SharedChild;

    #[derive(Debug)]
    pub struct TdxConfig {
        /// Guest CID for vhost-vsock
        pub cid: u32,
    }

    #[derive(Debug, Builder)]
    pub struct VmConfig {
        pub manifest: Manifest,
        pub image: Image,
        pub tdx_config: Option<TdxConfig>,
        pub networking: Networking,
    }

    fn create_hd(
        image_file: impl AsRef<Path>,
        backing_file: Option<impl AsRef<Path>>,
        size: &str,
    ) -> Result<()> {
        let mut command = Command::new("qemu-img");
        command.arg("create").arg("-f").arg("qcow2");
        if let Some(backing_file) = backing_file {
            command
                .arg("-o")
                .arg(format!("backing_file={}", backing_file.as_ref().display()));
            command.arg("-o").arg("backing_fmt=qcow2");
        }
        command.arg(image_file.as_ref());
        command.arg(size);
        command.spawn()?.wait()?;
        Ok(())
    }

    pub fn run_vm(
        qemu: &Path,
        config: &VmConfig,
        workdir: impl AsRef<Path>,
    ) -> Result<Arc<SharedChild>> {
        let workdir = workdir.as_ref();
        let serial_file = workdir.join("serial.log");
        let shared_dir = workdir.join("shared");
        let disk_size = format!("{}G", config.manifest.disk_size);
        let hda_path = workdir.join("hda.img");
        if !hda_path.exists() {
            create_hd(&hda_path, config.image.hda.as_ref(), &disk_size)?;
        }
        if !shared_dir.exists() {
            fs::create_dir_all(&shared_dir)?;
        }
        let mut command = Command::new(qemu);
        command.arg("-accel").arg("kvm");
        command.arg("-cpu").arg("host");
        command.arg("-smp").arg(config.manifest.vcpu.to_string());
        command
            .arg("-m")
            .arg(format!("{}M", config.manifest.memory));
        command.arg("-nographic");
        command.arg("-nodefaults");
        command
            .arg("-serial")
            .arg(format!("file:{}", serial_file.display()));
        command.arg("-kernel").arg(&config.image.kernel);
        command.arg("-initrd").arg(&config.image.initrd);
        command
            .arg("-drive")
            .arg(format!("file={},if=none,id=hd0", hda_path.display()))
            .arg("-device")
            .arg(format!("virtio-blk-pci,drive=hd0"));
        if let Some(rootfs) = &config.image.rootfs {
            command.arg("-cdrom").arg(rootfs);
        }
        if let Some(bios) = &config.image.bios {
            command.arg("-bios").arg(bios);
        }
        let netdev = match &config.networking {
            Networking::User(netcfg) => {
                let mut netdev = format!(
                    "user,id=net0,net={},dhcp-start={},restrict={}",
                    netcfg.net,
                    netcfg.dhcp_start,
                    if netcfg.restrict { "yes" } else { "no" }
                );
                for pm in &config.manifest.port_map {
                    netdev.push_str(&format!(
                        ",hostfwd={}:{}:{}-:{}",
                        pm.protocol.as_str(),
                        pm.address,
                        pm.from,
                        pm.to
                    ));
                }
                netdev
            }
            Networking::Custom(netcfg) => netcfg.netdev.clone(),
        };
        command.arg("-netdev").arg(netdev);
        command.arg("-device").arg("virtio-net-pci,netdev=net0");
        if let Some(tdx) = &config.tdx_config {
            command
                .arg("-machine")
                .arg("q35,kernel-irqchip=split,confidential-guest-support=tdx,hpet=off");
            command.arg("-object").arg("tdx-guest,id=tdx");
            command
                .arg("-device")
                .arg(format!("vhost-vsock-pci,guest-cid={}", tdx.cid));
        }
        command.arg("-virtfs").arg(format!(
            "local,path={},mount_tag=host-shared,readonly=off,security_model=mapped,id=virtfs0",
            shared_dir.display()
        ));
        if let Some(cmdline) = &config.image.info.cmdline {
            command.arg("-append").arg(cmdline);
        }
        command.current_dir(workdir);
        let child = SharedChild::spawn(&mut command)?;
        Ok(Arc::new(child))
    }

    pub fn get_log_file(workdir: impl AsRef<Path>) -> Result<PathBuf> {
        let serial_file = workdir.as_ref().join("serial.log");
        Ok(serial_file)
    }
}

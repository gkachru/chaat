use super::*;

impl Worker {
    pub(super) fn discard_update(&self, prepared: Box<fastframe_update::Prepared>) {
        tokio::task::spawn_blocking(move || prepared.discard());
    }

    pub(super) fn acknowledge_update(&self, receipt: fastframe_update::Receipt) {
        tokio::task::spawn_blocking(move || {
            if receipt.acknowledge().is_err() {
                log::warn!("could not acknowledge the app update");
            }
        });
    }

    pub(super) fn check_update(&self, download: bool) {
        let out = self.out.clone();
        tokio::task::spawn_blocking(move || {
            let result = crate::updates::updater().and_then(|updater| {
                match updater.check()? {
                    None => out.send(ServiceEvent::Update(crate::updates::Event::Status(
                        crate::updates::Status::Current,
                    ))),
                    Some(release) => {
                        let reason = updater.installation().err().map(|r| r.to_string());
                        if download && reason.is_none() {
                            download_update(&out, &updater, &release)?;
                        } else {
                            out.send(ServiceEvent::Update(crate::updates::Event::Status(
                                crate::updates::Status::Available { release, reason },
                            )));
                        }
                    }
                }
                Ok(())
            });
            if result.is_err() {
                out.send(ServiceEvent::Update(crate::updates::Event::Status(crate::updates::Status::Failed("Could not check for updates. No public release may be available yet; try again later."))));
            }
        });
    }

    pub(super) fn download_release(&self, release: fastframe_update::Release) {
        let out = self.out.clone();
        tokio::task::spawn_blocking(move || {
            if crate::updates::updater()
                .and_then(|updater| download_update(&out, &updater, &release))
                .is_err()
            {
                out.send(ServiceEvent::Update(crate::updates::Event::Status(crate::updates::Status::Failed("Could not download or verify the update. Your installed app has not changed."))));
            }
        });
    }

    pub(super) fn install_update(&self, prepared: Box<fastframe_update::Prepared>) {
        let out = self.out.clone();
        tokio::task::spawn_blocking(move || {
            if crate::updates::updater()
                .and_then(|updater| updater.handoff(*prepared, Vec::new()))
                .is_ok()
            {
                out.send(ServiceEvent::Update(crate::updates::Event::Quit));
            } else {
                out.send(ServiceEvent::Update(crate::updates::Event::Status(
                    crate::updates::Status::Failed(
                        "Could not start the update. Your installed app has not changed.",
                    ),
                )));
            }
        });
    }
}

fn download_update(
    out: &Outbox,
    updater: &fastframe_update::Updater,
    release: &fastframe_update::Release,
) -> anyhow::Result<()> {
    out.send(ServiceEvent::Update(crate::updates::Event::Status(
        crate::updates::Status::Downloading {
            received: 0,
            total: 0,
        },
    )));
    let mut last_percent = 0;
    let prepared = updater.download(release, |received, total| {
        let percent = received.saturating_mul(100).checked_div(total).unwrap_or(0);
        if percent > last_percent || received == total {
            last_percent = percent;
            out.send(ServiceEvent::Update(crate::updates::Event::Status(
                crate::updates::Status::Downloading { received, total },
            )));
        }
    })?;
    out.send(ServiceEvent::Update(crate::updates::Event::Status(
        crate::updates::Status::Ready(Box::new(prepared)),
    )));
    Ok(())
}

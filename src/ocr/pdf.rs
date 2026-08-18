use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Context, Result};
use tempfile::TempDir;
use tokio::{fs, process::Command};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageRange {
    pub start: u32,
    pub end: u32,
}

impl FromStr for PageRange {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        anyhow::ensure!(!value.is_empty(), "page_range cannot be empty");

        let (start, end) = match value.split_once('-') {
            Some((start, end)) => (parse_page(start)?, parse_page(end)?),
            None => {
                let page = parse_page(value)?;
                (page, page)
            }
        };
        anyhow::ensure!(start <= end, "page_range start must be <= end");

        Ok(Self { start, end })
    }
}

fn parse_page(value: &str) -> Result<u32> {
    let page: u32 = value
        .trim()
        .parse()
        .with_context(|| format!("invalid page number {value:?}"))?;
    anyhow::ensure!(page > 0, "page numbers start at 1");
    Ok(page)
}

#[derive(Debug)]
pub struct PageImage {
    pub number: u32,
    pub path: PathBuf,
}

pub struct PdfPages {
    _temp_dir: TempDir,
    pub pages: Vec<PageImage>,
}

impl PdfPages {
    pub async fn render(pdf: &[u8], page_range: Option<PageRange>, dpi: u16) -> Result<Self> {
        anyhow::ensure!(!pdf.is_empty(), "PDF is empty");
        anyhow::ensure!(dpi > 0, "PDF DPI must be greater than zero");

        let temp_dir = tempfile::tempdir().context("failed to create PDF working directory")?;
        let pdf_path = temp_dir.path().join("input.pdf");
        let output_prefix = temp_dir.path().join("page");
        fs::write(&pdf_path, pdf)
            .await
            .context("failed to write uploaded PDF")?;

        let mut command = Command::new("pdftoppm");
        command.arg("-png").arg("-r").arg(dpi.to_string());
        if let Some(range) = page_range {
            command
                .arg("-f")
                .arg(range.start.to_string())
                .arg("-l")
                .arg(range.end.to_string());
        }
        let output = command.arg(&pdf_path).arg(&output_prefix).output().await;
        let output =
            output.context("failed to run pdftoppm; install poppler and ensure it is on PATH")?;
        if !output.status.success() {
            anyhow::bail!(
                "pdftoppm failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }

        let mut entries = fs::read_dir(temp_dir.path())
            .await
            .context("failed to list rendered PDF pages")?;
        let mut pages = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(number) = rendered_page_number(&path) {
                pages.push(PageImage { number, path });
            }
        }
        pages.sort_by_key(|page| page.number);
        anyhow::ensure!(!pages.is_empty(), "pdftoppm produced no pages");

        Ok(Self {
            _temp_dir: temp_dir,
            pages,
        })
    }
}

fn rendered_page_number(path: &Path) -> Option<u32> {
    if path.extension()?.to_str()? != "png" {
        return None;
    }
    path.file_stem()?
        .to_str()?
        .strip_prefix("page-")?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use std::{path::Path, str::FromStr};

    use super::{PageRange, rendered_page_number};

    #[test]
    fn parses_single_page_and_range() {
        assert_eq!(
            PageRange::from_str("3").unwrap(),
            PageRange { start: 3, end: 3 }
        );
        assert_eq!(
            PageRange::from_str(" 2 - 7 ").unwrap(),
            PageRange { start: 2, end: 7 }
        );
        assert!(PageRange::from_str("7-2").is_err());
        assert!(PageRange::from_str("0").is_err());
    }

    #[test]
    fn extracts_page_number_from_rendered_filename() {
        assert_eq!(
            rendered_page_number(Path::new("/tmp/page-12.png")),
            Some(12)
        );
        assert_eq!(rendered_page_number(Path::new("/tmp/input.pdf")), None);
    }
}

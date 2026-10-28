use std::fs::File;
use std::io::Write;

use anyhow::{bail, Result};
use bumpalo::Bump;
use memmap2::Mmap;
use milli::heed::EnvOpenOptions;
use milli::progress::Progress;
use milli::update::new::indexer;
use milli::update::IndexerConfig;
use milli::{FieldsIdsMap, Index, MustStopProcessing};
use serde::de::DeserializeOwned;
use tempfile::{tempdir, tempfile, TempDir};

use crate::App;

const MAP_SIZE: usize = 1024 * 1024 * 1024;

pub(crate) struct Search {
    index: Index,
    _index_directory: TempDir,
}

impl Search {
    pub(crate) fn build(apps: &[App]) -> Result<Self> {
        let index_directory = tempdir()?;
        let index = Index::new(open_options(), index_directory.path())?;

        configure(&index)?;
        let documents = write_documents(apps)?;
        index_documents(&index, &documents)?;

        let transaction = index.read_txn()?;
        let indexed_count = index.documents_ids(&transaction)?.len();
        if indexed_count != apps.len() as u64 {
            bail!("indexed {indexed_count} apps, expected {}", apps.len());
        }
        drop(transaction);

        Ok(Self {
            index,
            _index_directory: index_directory,
        })
    }

    pub(crate) fn query(&self, query: &str, limit: usize) -> Result<Vec<App>> {
        let transaction = self.index.read_txn()?;
        let fields = self.index.fields_ids_map(&transaction)?;
        let progress = Progress::default();
        let mut search = self.index.search(&transaction, &fields, &progress);
        search.query(query).limit(limit);

        let matches = search.execute()?;
        let documents = self
            .index
            .documents(&transaction, matches.documents_ids.iter().copied())?;

        documents
            .into_iter()
            .map(|(_, document)| {
                Ok(App {
                    id: field(document, &fields, "id")?,
                    name: field(document, &fields, "name")?,
                    description: field(document, &fields, "description")?,
                    package_source: field(document, &fields, "package_source")?,
                    traffic: field(document, &fields, "traffic")?,
                })
            })
            .collect()
    }
}

fn open_options() -> milli::heed::EnvOpenOptions<milli::heed::WithoutTls> {
    let options = EnvOpenOptions::new();
    let mut options = options.read_txn_without_tls();
    options.map_size(MAP_SIZE);
    options
}

fn configure(index: &Index) -> Result<()> {
    let mut transaction = index.write_txn()?;
    milli::fixed_config::configure(index, &mut transaction)?;
    transaction.commit()?;
    Ok(())
}

fn write_documents(apps: &[App]) -> Result<File> {
    let mut documents = tempfile()?;
    for app in apps {
        serde_json::to_writer(&mut documents, app)?;
        documents.write_all(b"\n")?;
    }
    Ok(documents)
}

fn index_documents(index: &Index, documents: &File) -> Result<()> {
    let documents = unsafe { Mmap::map(documents)? };
    let config = IndexerConfig::default();
    let transaction = index.read_txn()?;
    let mut write_transaction = index.write_txn()?;
    let current_fields = index.fields_ids_map(&transaction)?;
    let mut new_fields = current_fields.clone();
    let mut operations = indexer::IndexOperations::new();
    operations.replace_documents(&documents)?;

    let allocator = Bump::new();
    let (changes, statistics, primary_key) = operations.into_changes(
        &allocator,
        index,
        &transaction,
        None,
        &mut new_fields,
        &MustStopProcessing::default(),
        Progress::default(),
    )?;
    if let Some(error) = statistics.into_iter().find_map(|statistic| statistic.error) {
        return Err(error.into());
    }

    config
        .thread_pool
        .install(|| {
            indexer::index(
                &mut write_transaction,
                index,
                &config.thread_pool,
                config.grenad_parameters(),
                &current_fields,
                new_fields,
                primary_key,
                &changes,
                &MustStopProcessing::default(),
                &Progress::default(),
            )
        })
        .expect("indexing thread panicked")?;

    write_transaction.commit()?;
    Ok(())
}

fn field<T: DeserializeOwned>(
    document: &milli::update::new::KvReaderFieldId,
    fields: &FieldsIdsMap,
    name: &'static str,
) -> Result<T> {
    let Some(field_id) = fields.id(name) else {
        bail!("search index has no {name} field");
    };
    let Some(value) = document.get(field_id) else {
        bail!("search result has no {name} field");
    };
    Ok(serde_json::from_slice(value)?)
}

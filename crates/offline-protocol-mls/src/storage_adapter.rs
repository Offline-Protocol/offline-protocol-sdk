use crate::error::MlsError;
use crate::storage::MlsStorage;
use openmls_traits::storage::{traits, StorageProvider};
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;

/// Adapter to make MlsStorage compatible with OpenMLS StorageProvider.
#[derive(Clone)]
pub struct MlsStorageAdapter {
    storage: Arc<dyn MlsStorage>,
}

impl MlsStorageAdapter {
    pub fn new(storage: Arc<dyn MlsStorage>) -> Self {
        Self { storage }
    }

    fn write_generic<K: Serialize, V: Serialize + ?Sized>(
        &self,
        label: &str,
        key: &K,
        value: &V,
    ) -> Result<(), MlsError> {
        let key_bytes =
            serde_json::to_vec(key).map_err(|e| MlsError::Serialization(e.to_string()))?;
        let key_id = hex::encode(key_bytes);
        let value_bytes =
            serde_json::to_vec(value).map_err(|e| MlsError::Serialization(e.to_string()))?;
        self.storage.store(label, &key_id, &value_bytes)?;
        Ok(())
    }

    fn read_generic<K: Serialize, V: DeserializeOwned>(
        &self,
        label: &str,
        key: &K,
    ) -> Result<Option<V>, MlsError> {
        let key_bytes =
            serde_json::to_vec(key).map_err(|e| MlsError::Serialization(e.to_string()))?;
        let key_id = hex::encode(key_bytes);
        let data = self.storage.load(label, &key_id)?;
        match data {
            Some(bytes) => {
                let v = serde_json::from_slice(&bytes)
                    .map_err(|e| MlsError::Deserialization(e.to_string()))?;
                Ok(Some(v))
            }
            None => Ok(None),
        }
    }

    fn delete_generic<K: Serialize>(&self, label: &str, key: &K) -> Result<(), MlsError> {
        let key_bytes =
            serde_json::to_vec(key).map_err(|e| MlsError::Serialization(e.to_string()))?;
        let key_id = hex::encode(key_bytes);
        self.storage.delete(label, &key_id)?;
        Ok(())
    }
}

impl<const VERSION: u16> StorageProvider<VERSION> for MlsStorageAdapter {
    type Error = MlsError;

    fn write_mls_join_config<
        GroupId: traits::GroupId<VERSION>,
        MlsGroupJoinConfig: traits::MlsGroupJoinConfig<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        config: &MlsGroupJoinConfig,
    ) -> Result<(), Self::Error> {
        self.write_generic("mls_join_config", group_id, config)
    }

    fn append_own_leaf_node<
        GroupId: traits::GroupId<VERSION>,
        LeafNode: traits::LeafNode<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        leaf_node: &LeafNode,
    ) -> Result<(), Self::Error> {
        let label = "own_leaf_nodes";
        let mut nodes: Vec<LeafNode> = self.read_generic(label, group_id)?.unwrap_or_default();
        nodes.push(serde_json::from_slice(&serde_json::to_vec(leaf_node).unwrap()).unwrap());
        self.write_generic(label, group_id, &nodes)
    }

    fn queue_proposal<
        GroupId: traits::GroupId<VERSION>,
        ProposalRef: traits::ProposalRef<VERSION>,
        QueuedProposal: traits::QueuedProposal<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        proposal_ref: &ProposalRef,
        proposal: &QueuedProposal,
    ) -> Result<(), Self::Error> {
        let label = "queued_proposals";
        let mut list: Vec<(ProposalRef, QueuedProposal)> =
            self.read_generic(label, group_id)?.unwrap_or_default();

        let ref_clone = serde_json::from_slice(&serde_json::to_vec(proposal_ref).unwrap()).unwrap();
        let prop_clone = serde_json::from_slice(&serde_json::to_vec(proposal).unwrap()).unwrap();

        list.push((ref_clone, prop_clone));
        self.write_generic(label, group_id, &list)
    }

    fn write_tree<GroupId: traits::GroupId<VERSION>, TreeSync: traits::TreeSync<VERSION>>(
        &self,
        group_id: &GroupId,
        tree: &TreeSync,
    ) -> Result<(), Self::Error> {
        self.write_generic("tree", group_id, tree)
    }

    fn write_interim_transcript_hash<
        GroupId: traits::GroupId<VERSION>,
        InterimTranscriptHash: traits::InterimTranscriptHash<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        interim_transcript_hash: &InterimTranscriptHash,
    ) -> Result<(), Self::Error> {
        self.write_generic("interim_transcript_hash", group_id, interim_transcript_hash)
    }

    fn write_context<
        GroupId: traits::GroupId<VERSION>,
        GroupContext: traits::GroupContext<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        group_context: &GroupContext,
    ) -> Result<(), Self::Error> {
        self.write_generic("group_context", group_id, group_context)
    }

    fn write_confirmation_tag<
        GroupId: traits::GroupId<VERSION>,
        ConfirmationTag: traits::ConfirmationTag<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        confirmation_tag: &ConfirmationTag,
    ) -> Result<(), Self::Error> {
        self.write_generic("confirmation_tag", group_id, confirmation_tag)
    }

    fn write_group_state<
        GroupState: traits::GroupState<VERSION>,
        GroupId: traits::GroupId<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        group_state: &GroupState,
    ) -> Result<(), Self::Error> {
        self.write_generic("openmls_group_state", group_id, group_state)
    }

    fn write_message_secrets<
        GroupId: traits::GroupId<VERSION>,
        MessageSecrets: traits::MessageSecrets<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        message_secrets: &MessageSecrets,
    ) -> Result<(), Self::Error> {
        self.write_generic("message_secrets", group_id, message_secrets)
    }

    fn write_resumption_psk_store<
        GroupId: traits::GroupId<VERSION>,
        ResumptionPskStore: traits::ResumptionPskStore<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        resumption_psk_store: &ResumptionPskStore,
    ) -> Result<(), Self::Error> {
        self.write_generic("resumption_psk_store", group_id, resumption_psk_store)
    }

    fn write_own_leaf_index<
        GroupId: traits::GroupId<VERSION>,
        LeafNodeIndex: traits::LeafNodeIndex<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        own_leaf_index: &LeafNodeIndex,
    ) -> Result<(), Self::Error> {
        self.write_generic("own_leaf_index", group_id, own_leaf_index)
    }

    fn write_group_epoch_secrets<
        GroupId: traits::GroupId<VERSION>,
        GroupEpochSecrets: traits::GroupEpochSecrets<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        group_epoch_secrets: &GroupEpochSecrets,
    ) -> Result<(), Self::Error> {
        self.write_generic("group_epoch_secrets", group_id, group_epoch_secrets)
    }

    fn write_signature_key_pair<
        SignaturePublicKey: traits::SignaturePublicKey<VERSION>,
        SignatureKeyPair: traits::SignatureKeyPair<VERSION>,
    >(
        &self,
        public_key: &SignaturePublicKey,
        signature_key_pair: &SignatureKeyPair,
    ) -> Result<(), Self::Error> {
        self.write_generic("signature_key_pair", public_key, signature_key_pair)
    }

    fn write_encryption_key_pair<
        EncryptionKey: traits::EncryptionKey<VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<VERSION>,
    >(
        &self,
        public_key: &EncryptionKey,
        key_pair: &HpkeKeyPair,
    ) -> Result<(), Self::Error> {
        self.write_generic("encryption_key_pair", public_key, key_pair)
    }

    fn write_encryption_epoch_key_pairs<
        GroupId: traits::GroupId<VERSION>,
        EpochKey: traits::EpochKey<VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
        key_pairs: &[HpkeKeyPair],
    ) -> Result<(), Self::Error> {
        #[derive(Serialize)]
        struct Key<'a, G, E> {
            group_id: &'a G,
            epoch: &'a E,
            leaf_index: u32,
        }
        let key = Key {
            group_id,
            epoch,
            leaf_index,
        };
        self.write_generic("encryption_epoch_key_pairs", &key, key_pairs)
    }

    fn write_key_package<
        HashReference: traits::HashReference<VERSION>,
        KeyPackage: traits::KeyPackage<VERSION>,
    >(
        &self,
        hash_ref: &HashReference,
        key_package: &KeyPackage,
    ) -> Result<(), Self::Error> {
        let key_bytes =
            serde_json::to_vec(hash_ref).map_err(|e| MlsError::Serialization(e.to_string()))?;
        let key_id = hex::encode(&key_bytes);
        tracing::info!(
            key_id = %key_id,
            key_bytes_len = key_bytes.len(),
            "write_key_package: storing key package"
        );
        self.write_generic("openmls_key_package", hash_ref, key_package)
    }

    fn write_psk<PskId: traits::PskId<VERSION>, PskBundle: traits::PskBundle<VERSION>>(
        &self,
        psk_id: &PskId,
        psk: &PskBundle,
    ) -> Result<(), Self::Error> {
        self.write_generic("psk", psk_id, psk)
    }

    fn mls_group_join_config<
        GroupId: traits::GroupId<VERSION>,
        MlsGroupJoinConfig: traits::MlsGroupJoinConfig<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<MlsGroupJoinConfig>, Self::Error> {
        self.read_generic("mls_join_config", group_id)
    }

    fn own_leaf_nodes<GroupId: traits::GroupId<VERSION>, LeafNode: traits::LeafNode<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<LeafNode>, Self::Error> {
        Ok(self
            .read_generic("own_leaf_nodes", group_id)?
            .unwrap_or_default())
    }

    fn queued_proposal_refs<
        GroupId: traits::GroupId<VERSION>,
        ProposalRef: traits::ProposalRef<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<ProposalRef>, Self::Error> {
        let proposals: Vec<(ProposalRef, serde_json::Value)> = self
            .read_generic("queued_proposals", group_id)?
            .unwrap_or_default();
        Ok(proposals.into_iter().map(|(r, _)| r).collect())
    }

    fn queued_proposals<
        GroupId: traits::GroupId<VERSION>,
        ProposalRef: traits::ProposalRef<VERSION>,
        QueuedProposal: traits::QueuedProposal<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Vec<(ProposalRef, QueuedProposal)>, Self::Error> {
        Ok(self
            .read_generic("queued_proposals", group_id)?
            .unwrap_or_default())
    }

    fn tree<GroupId: traits::GroupId<VERSION>, TreeSync: traits::TreeSync<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<TreeSync>, Self::Error> {
        self.read_generic("tree", group_id)
    }

    fn group_context<
        GroupId: traits::GroupId<VERSION>,
        GroupContext: traits::GroupContext<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupContext>, Self::Error> {
        self.read_generic("group_context", group_id)
    }

    fn interim_transcript_hash<
        GroupId: traits::GroupId<VERSION>,
        InterimTranscriptHash: traits::InterimTranscriptHash<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<InterimTranscriptHash>, Self::Error> {
        self.read_generic("interim_transcript_hash", group_id)
    }

    fn confirmation_tag<
        GroupId: traits::GroupId<VERSION>,
        ConfirmationTag: traits::ConfirmationTag<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<ConfirmationTag>, Self::Error> {
        self.read_generic("confirmation_tag", group_id)
    }

    fn group_state<GroupState: traits::GroupState<VERSION>, GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupState>, Self::Error> {
        self.read_generic("openmls_group_state", group_id)
    }

    fn message_secrets<
        GroupId: traits::GroupId<VERSION>,
        MessageSecrets: traits::MessageSecrets<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<MessageSecrets>, Self::Error> {
        self.read_generic("message_secrets", group_id)
    }

    fn resumption_psk_store<
        GroupId: traits::GroupId<VERSION>,
        ResumptionPskStore: traits::ResumptionPskStore<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<ResumptionPskStore>, Self::Error> {
        self.read_generic("resumption_psk_store", group_id)
    }

    fn own_leaf_index<
        GroupId: traits::GroupId<VERSION>,
        LeafNodeIndex: traits::LeafNodeIndex<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<LeafNodeIndex>, Self::Error> {
        self.read_generic("own_leaf_index", group_id)
    }

    fn group_epoch_secrets<
        GroupId: traits::GroupId<VERSION>,
        GroupEpochSecrets: traits::GroupEpochSecrets<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<Option<GroupEpochSecrets>, Self::Error> {
        self.read_generic("group_epoch_secrets", group_id)
    }

    fn signature_key_pair<
        SignaturePublicKey: traits::SignaturePublicKey<VERSION>,
        SignatureKeyPair: traits::SignatureKeyPair<VERSION>,
    >(
        &self,
        public_key: &SignaturePublicKey,
    ) -> Result<Option<SignatureKeyPair>, Self::Error> {
        self.read_generic("signature_key_pair", public_key)
    }

    fn encryption_key_pair<
        HpkeKeyPair: traits::HpkeKeyPair<VERSION>,
        EncryptionKey: traits::EncryptionKey<VERSION>,
    >(
        &self,
        public_key: &EncryptionKey,
    ) -> Result<Option<HpkeKeyPair>, Self::Error> {
        self.read_generic("encryption_key_pair", public_key)
    }

    fn encryption_epoch_key_pairs<
        GroupId: traits::GroupId<VERSION>,
        EpochKey: traits::EpochKey<VERSION>,
        HpkeKeyPair: traits::HpkeKeyPair<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
    ) -> Result<Vec<HpkeKeyPair>, Self::Error> {
        #[derive(Serialize)]
        struct Key<'a, G, E> {
            group_id: &'a G,
            epoch: &'a E,
            leaf_index: u32,
        }
        let key = Key {
            group_id,
            epoch,
            leaf_index,
        };
        Ok(self
            .read_generic("encryption_epoch_key_pairs", &key)?
            .unwrap_or_default())
    }

    fn key_package<
        KeyPackageRef: traits::HashReference<VERSION>,
        KeyPackage: traits::KeyPackage<VERSION>,
    >(
        &self,
        hash_ref: &KeyPackageRef,
    ) -> Result<Option<KeyPackage>, Self::Error> {
        let key_bytes =
            serde_json::to_vec(hash_ref).map_err(|e| MlsError::Serialization(e.to_string()))?;
        let key_id = hex::encode(&key_bytes);
        let result: Result<Option<KeyPackage>, _> =
            self.read_generic("openmls_key_package", hash_ref);
        match &result {
            Ok(Some(_)) => tracing::info!(
                key_id = %key_id,
                "key_package: found key package"
            ),
            Ok(None) => {
                let stored_keys = self.storage.list_keys("openmls_key_package")
                    .unwrap_or_default();
                tracing::warn!(
                    key_id = %key_id,
                    stored_count = stored_keys.len(),
                    stored_keys = ?stored_keys,
                    "key_package: NOT FOUND in storage"
                );
            }
            Err(e) => tracing::error!(
                key_id = %key_id,
                error = %e,
                "key_package: error reading from storage"
            ),
        }
        result
    }

    fn psk<PskBundle: traits::PskBundle<VERSION>, PskId: traits::PskId<VERSION>>(
        &self,
        psk_id: &PskId,
    ) -> Result<Option<PskBundle>, Self::Error> {
        self.read_generic("psk", psk_id)
    }

    fn remove_proposal<
        GroupId: traits::GroupId<VERSION>,
        ProposalRef: traits::ProposalRef<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        proposal_ref: &ProposalRef,
    ) -> Result<(), Self::Error> {
        let label = "queued_proposals";
        let mut list: Vec<(ProposalRef, serde_json::Value)> =
            self.read_generic(label, group_id)?.unwrap_or_default();
        let ref_bytes = serde_json::to_vec(proposal_ref).unwrap();

        list.retain(|(r, _)| serde_json::to_vec(r).unwrap() != ref_bytes);

        self.write_generic(label, group_id, &list)
    }

    fn delete_own_leaf_nodes<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("own_leaf_nodes", group_id)
    }

    fn delete_group_config<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("mls_join_config", group_id)
    }

    fn delete_tree<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("tree", group_id)
    }

    fn delete_confirmation_tag<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("confirmation_tag", group_id)
    }

    fn delete_group_state<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("openmls_group_state", group_id)
    }

    fn delete_context<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("group_context", group_id)
    }

    fn delete_interim_transcript_hash<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("interim_transcript_hash", group_id)
    }

    fn delete_message_secrets<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("message_secrets", group_id)
    }

    fn delete_all_resumption_psk_secrets<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("resumption_psk_store", group_id)
    }

    fn delete_own_leaf_index<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("own_leaf_index", group_id)
    }

    fn delete_group_epoch_secrets<GroupId: traits::GroupId<VERSION>>(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("group_epoch_secrets", group_id)
    }

    fn clear_proposal_queue<
        GroupId: traits::GroupId<VERSION>,
        ProposalRef: traits::ProposalRef<VERSION>,
    >(
        &self,
        group_id: &GroupId,
    ) -> Result<(), Self::Error> {
        self.delete_generic("queued_proposals", group_id)
    }

    fn delete_signature_key_pair<SignaturePublicKey: traits::SignaturePublicKey<VERSION>>(
        &self,
        public_key: &SignaturePublicKey,
    ) -> Result<(), Self::Error> {
        self.delete_generic("signature_key_pair", public_key)
    }

    fn delete_encryption_key_pair<EncryptionKey: traits::EncryptionKey<VERSION>>(
        &self,
        public_key: &EncryptionKey,
    ) -> Result<(), Self::Error> {
        self.delete_generic("encryption_key_pair", public_key)
    }

    fn delete_encryption_epoch_key_pairs<
        GroupId: traits::GroupId<VERSION>,
        EpochKey: traits::EpochKey<VERSION>,
    >(
        &self,
        group_id: &GroupId,
        epoch: &EpochKey,
        leaf_index: u32,
    ) -> Result<(), Self::Error> {
        #[derive(Serialize)]
        struct Key<'a, G, E> {
            group_id: &'a G,
            epoch: &'a E,
            leaf_index: u32,
        }
        let key = Key {
            group_id,
            epoch,
            leaf_index,
        };
        self.delete_generic("encryption_epoch_key_pairs", &key)
    }

    fn delete_key_package<KeyPackageRef: traits::HashReference<VERSION>>(
        &self,
        hash_ref: &KeyPackageRef,
    ) -> Result<(), Self::Error> {
        let key_bytes =
            serde_json::to_vec(hash_ref).map_err(|e| MlsError::Serialization(e.to_string()))?;
        let key_id = hex::encode(&key_bytes);
        tracing::info!(
            key_id = %key_id,
            "delete_key_package: removing key package from storage"
        );
        self.delete_generic("openmls_key_package", hash_ref)
    }

    fn delete_psk<PskKey: traits::PskId<VERSION>>(
        &self,
        psk_id: &PskKey,
    ) -> Result<(), Self::Error> {
        self.delete_generic("psk", psk_id)
    }
}

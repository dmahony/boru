package com.example.chatapp.model

data class DownloadProgress(
    val state: DownloadState,
    val progressPercentage: Float,
    val totalSizeBytes: Long,
    val sourcePeerId: String,
    val speedBytesPerSecond: Long?,
    val failureReason: String?
)

sealed class DownloadAction {
    object Pause : DownloadAction()
    object Resume : DownloadAction()
    object Cancel : DownloadAction()
}

enum class DownloadState {
    PENDING,
    QUEUED,
    RESOLVING_PEER,
    REQUESTING_PERMISSION,
    DOWNLOADING,
    PAUSED,
    COMPLETED,
    FAILED,
    CANCELLED
}

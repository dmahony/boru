package com.example.chatapp.manager

import com.example.chatapp.model.DownloadProgress
import com.example.chatapp.model.DownloadState
import com.example.chatapp.repository.DownloadRepository
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

class DownloadManager(private val repository: DownloadRepository) {

    private val _downloadProgress = MutableStateFlow<DownloadProgress?>(null)
    val downloadProgress: StateFlow<DownloadProgress?> = _downloadProgress.asStateFlow()

    init {
        val lastProgress = repository.getDownloadProgress()
        if (lastProgress != null) {
            val newState = when (lastProgress.state) {
                DownloadState.RESOLVING_PEER, DownloadState.REQUESTING_PERMISSION -> {
                    if (repository.hasPartialData()) {
                        DownloadState.PAUSED
                    } else {
                        DownloadState.QUEUED
                    }
                }
                else -> lastProgress.state
            }
            _downloadProgress.value = lastProgress.copy(state = newState)
            saveState()
        }
    }

    fun startDownload(sourcePeerId: String, totalSizeBytes: Long) {
        _downloadProgress.value = DownloadProgress(
            state = DownloadState.QUEUED,
            progressPercentage = 0f,
            totalSizeBytes = totalSizeBytes,
            sourcePeerId = sourcePeerId,
            speedBytesPerSecond = null,
            failureReason = null
        )
        saveState()
        // Simulate download starting
        _downloadProgress.update { it?.copy(state = DownloadState.DOWNLOADING) }
        saveState()
    }

    fun pauseDownload() {
        _downloadProgress.update {
            it?.copy(state = DownloadState.PAUSED, speedBytesPerSecond = 0)
        }
        saveState()
    }

    fun resumeDownload() {
        _downloadProgress.update {
            it?.copy(state = DownloadState.DOWNLOADING)
        }
        saveState()
    }

    fun cancelDownload() {
        _downloadProgress.update {
            it?.copy(state = DownloadState.CANCELLED, progressPercentage = 0f, speedBytesPerSecond = 0)
        }
        saveState()
    }

    fun failDownload(reason: String) {
        _downloadProgress.update {
            it?.copy(state = DownloadState.FAILED, failureReason = reason, speedBytesPerSecond = 0)
        }
        saveState()
    }

    private fun saveState() {
        _downloadProgress.value?.let { repository.saveDownloadProgress(it) }
    }

    companion object {
        @Volatile
        private var instance: DownloadManager? = null

        fun getInstance(repository: DownloadRepository): DownloadManager {
            return instance ?: synchronized(this) {
                instance ?: DownloadManager(repository).also { instance = it }
            }
        }
    }
}

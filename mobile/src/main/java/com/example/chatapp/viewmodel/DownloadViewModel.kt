package com.example.chatapp.viewmodel

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import com.example.chatapp.manager.DownloadManager
import com.example.chatapp.model.DownloadAction
import com.example.chatapp.model.DownloadProgress
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch

class DownloadViewModel(private val downloadManager: DownloadManager) : ViewModel() {

    val downloadProgress: StateFlow<DownloadProgress?> = downloadManager.downloadProgress

    fun onAction(action: DownloadAction) {
        viewModelScope.launch {
            when (action) {
                DownloadAction.Pause -> downloadManager.pauseDownload()
                DownloadAction.Resume -> downloadManager.resumeDownload()
                DownloadAction.Cancel -> downloadManager.cancelDownload()
            }
        }
    }

    fun startDownload(sourcePeerId: String, totalSizeBytes: Long) {
        viewModelScope.launch {
            downloadManager.startDownload(sourcePeerId, totalSizeBytes)
        }
    }

    fun failDownload(reason: String) {
        viewModelScope.launch {
            downloadManager.failDownload(reason)
        }
    }
}

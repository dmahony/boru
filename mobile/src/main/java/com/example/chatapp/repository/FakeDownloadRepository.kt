package com.example.chatapp.repository

import com.example.chatapp.model.DownloadProgress
import com.example.chatapp.model.DownloadState

class FakeDownloadRepository : DownloadRepository {
    private var downloadProgress: DownloadProgress? = null
    private var hasPartialData = false

    override fun getDownloadProgress(): DownloadProgress? {
        return downloadProgress
    }

    override fun saveDownloadProgress(downloadProgress: DownloadProgress) {
        this.downloadProgress = downloadProgress
    }

    override fun hasPartialData(): Boolean {
        return hasPartialData
    }

    fun setHasPartialData(hasPartialData: Boolean) {
        this.hasPartialData = hasPartialData
    }
}

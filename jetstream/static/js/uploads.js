/**
 * JetStream - Uploads Page
 * Upload form, folder analysis, recent jobs
 */

document.addEventListener('DOMContentLoaded', function() {
    loadStats();
    loadJobs();

    // Issue #8: live GCS destination preview
    document.getElementById('gcs-destination').addEventListener('input', updateGCSPreview);
    updateGCSPreview();

    // Refresh when jobs change (live updates), with polling as a fallback
    startLiveRefresh(function() {
        loadStats();
        loadJobs();
    });

    document.getElementById('dry-run').addEventListener('change', updateSubmitLabel);
    document.getElementById('upload-tool').addEventListener('change', updateUploadToolHelp);
    updateSubmitLabel();
    loadPresets();
    loadUploadTools();
});

// ===== UPLOAD TOOL CHOICE =====

let uploadTools = [];

async function loadUploadTools() {
    try {
        const response = await fetch('/api/system/upload-tools');
        if (!response.ok) return;
        uploadTools = await response.json();
    } catch (e) {
        return;
    }
    const select = document.getElementById('upload-tool');
    uploadTools.forEach(tool => {
        const option = select.querySelector('option[value="' + tool.id + '"]');
        if (!option) return;
        option.disabled = !tool.available;
        option.textContent = tool.label + (tool.id === 'gcloud' ? ' (recommended)' : '') + (tool.available ? '' : ' — not installed');
    });
    // Preferred default from Settings, if it's installed here.
    const preferred = getUIPreferences().default_upload_tool;
    const preferredTool = uploadTools.find(t => t.id === preferred && t.available);
    if (preferredTool) {
        select.value = preferredTool.id;
    } else if (select.selectedOptions[0] && select.selectedOptions[0].disabled) {
        const firstAvailable = uploadTools.find(t => t.available);
        if (firstAvailable) select.value = firstAvailable.id;
    }
    updateUploadToolHelp();
}

function updateUploadToolHelp() {
    const select = document.getElementById('upload-tool');
    const tool = uploadTools.find(t => t.id === select.value);
    const help = document.getElementById('upload-tool-help');
    if (!tool) {
        help.textContent = '';
        return;
    }
    help.textContent = tool.description + (tool.available ? '' : ' ' + (tool.detail || ''));
}

function submitLabel() {
    return document.getElementById('dry-run').checked ? '🔍 Start Preview' : '🚀 Start Upload';
}

function updateSubmitLabel() {
    document.getElementById('upload-submit').textContent = submitLabel();
}

// ===== UPLOAD FORM =====

document.getElementById('upload-form').addEventListener('submit', async function(e) {
    e.preventDefault();

    const scheduleTime = document.getElementById('schedule-time').value;
    let scheduled_for = null;

    if (scheduleTime) {
        const localDate = new Date(scheduleTime);
        const tzOffset = -localDate.getTimezoneOffset();
        const tzHours = Math.floor(Math.abs(tzOffset) / 60);
        const tzMinutes = Math.abs(tzOffset) % 60;
        const tzSign = tzOffset >= 0 ? '+' : '-';
        const tzString = tzSign + String(tzHours).padStart(2, '0') + ':' + String(tzMinutes).padStart(2, '0');
        scheduled_for = scheduleTime + ':00' + tzString;
    }

    // Parse advanced options
    const excludeFolders = document.getElementById('exclude-folders').value
        .split(',')
        .map(s => s.trim())
        .filter(s => s.length > 0);

    const excludePatterns = document.getElementById('exclude-patterns').value
        .split('\n')
        .map(s => s.trim())
        .filter(s => s.length > 0);

    const data = {
        source_path: document.getElementById('source-path').value,
        gcs_destination: document.getElementById('gcs-destination').value,
        threads: 4,
        recursive: document.getElementById('recursive').checked,
        split_by_folder: document.getElementById('split-by-folder').checked,
        dry_run: document.getElementById('dry-run').checked,
        upload_tool: document.getElementById('upload-tool').value,
        scheduled_for: scheduled_for,
        exclude_folders: excludeFolders.length > 0 ? excludeFolders : null,
        exclude_patterns: excludePatterns.length > 0 ? excludePatterns : null,
        // Issue #13: data protection options
        no_clobber: document.getElementById('no-clobber').checked,
        // Issue #14: auto-retry options
        auto_retry: document.getElementById('auto-retry').checked,
        auto_retry_delay_minutes: parseInt(document.getElementById('auto-retry-delay').value) || 30,
        max_auto_retries: parseInt(document.getElementById('max-auto-retries').value) || 3,
    };

    const submitBtn = e.target.querySelector('button[type="submit"]');
    setButtonLoading(submitBtn, true);

    try {
        const response = await fetch('/api/uploads/', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(data)
        });

        const result = await response.json();

        if (response.ok) {
            requestNotificationPermission();
            showToast((data.dry_run ? 'Preview started: ' : 'Upload started: ') + result.job_id, 'success');
            loadStats();
            loadJobs();
        } else {
            showToast('Error: ' + (result.detail || 'Unknown error'), 'error');
        }
    } catch (error) {
        showToast('Error: ' + error.message, 'error');
    } finally {
        setButtonLoading(submitBtn, false, submitLabel());
    }
});

// ===== STATS =====

function loadStats() {
    fetch('/api/stats/')
        .then(response => response.json())
        .then(data => {
            document.getElementById('total-jobs').textContent = data.total_jobs || 0;
            document.getElementById('active-jobs').textContent = data.active_jobs || 0;
            document.getElementById('data-uploaded').textContent = (data.total_uploaded_gb || 0).toFixed(2) + ' GB';
            document.getElementById('queue-length').textContent = data.queue_length || 0;
        })
        .catch(error => console.error('Error loading stats:', error));
}

// ===== RECENT JOBS =====

function clearCompletedJobs() {
    if (!confirm('This will hide all completed, failed, and cancelled jobs from the recent list. Continue?')) {
        return;
    }
    fetch('/api/uploads/clear-completed', { method: 'POST' })
        .then(response => response.json())
        .then(data => {
            showToast(data.message, 'success');
            loadJobs();
        })
        .catch(error => {
            console.error('Error clearing jobs:', error);
            showToast('Failed to clear jobs', 'error');
        });
}

function loadJobs() {
    const jobsList = document.getElementById('jobs-list');
    // Only show spinner if list is empty or has placeholder
    if (!jobsList.querySelector('.job-item')) {
        showSpinner('jobs-list', 'Loading recent jobs...');
    }
    fetch('/api/uploads/?limit=10')
        .then(response => response.json())
        .then(data => {
            // API returns flat array, not {jobs: [...]}
            const jobs = Array.isArray(data) ? data : [];
            if (jobs.length === 0) {
                jobsList.innerHTML = '<p style="text-align: center; color: #999; padding: 50px 0;">No jobs yet. Create your first upload!</p>';
                return;
            }

            jobsList.innerHTML = jobs.slice(0, 10).map(job => renderJobCard(job)).join('');
            updateLastRefreshed('uploads-last-updated');
        })
        .catch(error => console.error('Error loading jobs:', error));
}

// ===== FOLDER ANALYSIS =====

async function analyzeFolder() {
    const sourcePath = document.getElementById('source-path').value;
    if (!sourcePath) {
        showToast('Please enter a source path first', 'warning');
        return;
    }

    const recursive = document.getElementById('recursive').checked;
    const analysisResult = document.getElementById('analysis-result');
    const analysisContent = document.getElementById('analysis-content');

    analysisContent.innerHTML = '<p>Analyzing...</p>';
    analysisResult.style.display = 'block';

    try {
        const response = await fetch('/api/folders/analyze', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                path: sourcePath,
                recursive: recursive
            })
        });

        const data = await response.json();

        if (response.ok) {
            analysisContent.innerHTML =
                '<p><strong>Total Files:</strong> ' + data.total_files + '</p>' +
                '<p><strong>Total Size:</strong> ' + data.total_size_gb + ' GB</p>' +
                '<p><strong>Subfolders:</strong> ' + data.subfolder_count + '</p>';
        } else {
            analysisContent.innerHTML = '<p style="color: red;">Error: ' + data.detail + '</p>';
        }
    } catch (error) {
        analysisContent.innerHTML = '<p style="color: red;">Error: ' + error.message + '</p>';
    }
}

// ===== ADVANCED OPTIONS =====

function toggleAdvancedOptions() {
    const advancedOptions = document.getElementById('advanced-options');
    const toggleIcon = document.getElementById('advanced-toggle');

    if (advancedOptions.style.display === 'none') {
        advancedOptions.style.display = 'block';
        toggleIcon.textContent = '\u25b2';
    } else {
        advancedOptions.style.display = 'none';
        toggleIcon.textContent = '\u25bc';
    }
}

// Preset definitions for exclude patterns
const EXCLUDE_PRESETS = {
    temp: {
        folders: [],
        patterns: ['*.tmp', '*.bak', '*.temp', '~*', '*.swp']
    },
    system: {
        folders: ['__pycache__', '.git', 'node_modules', '.vs', '.idea'],
        patterns: ['Thumbs.db', '.DS_Store', 'desktop.ini', '*.pyc']
    },
    jpgonly: {
        folders: [],
        // Negative lookahead: exclude everything that doesn't end with .JPG or .jpg
        patterns: ['^.*(?<!\\.JPG)(?<!\\.jpg)$']
    },
    noraw: {
        folders: [],
        patterns: ['*.CR2', '*.CR3', '*.NEF', '*.ARW', '*.RAF', '*.ORF', '*.RW2', '*.DNG']
    },
    pifsc: {
        folders: ['_archive', '_YEAR', 'ISLAND', 'SITE-ID', 'SITE_PHOTOS', 'uncorrected', 'MISC', 'DARK', 'Products'],
        patterns: []
    }
};

function addExcludePreset(presetName) {
    const preset = EXCLUDE_PRESETS[presetName];
    if (!preset) return;
    
    // Add folders
    if (preset.folders.length > 0) {
        const foldersInput = document.getElementById('exclude-folders');
        const existingFolders = foldersInput.value.split(',').map(s => s.trim()).filter(s => s);
        const newFolders = preset.folders.filter(f => !existingFolders.includes(f));
        if (newFolders.length > 0) {
            foldersInput.value = existingFolders.concat(newFolders).join(', ');
        }
    }
    
    // Add patterns
    if (preset.patterns.length > 0) {
        const patternsTextarea = document.getElementById('exclude-patterns');
        const existingPatterns = patternsTextarea.value.split('\n').map(s => s.trim()).filter(s => s);
        const newPatterns = preset.patterns.filter(p => !existingPatterns.includes(p));
        if (newPatterns.length > 0) {
            patternsTextarea.value = existingPatterns.concat(newPatterns).join('\n');
        }
    }
    
    showToast('Added ' + presetName + ' preset', 'success');
}

function clearExcludePatterns() {
    document.getElementById('exclude-folders').value = '';
    document.getElementById('exclude-patterns').value = '';
    showToast('Cleared all exclude patterns', 'info');
}

// ===== GCS PATH PREVIEW (Issue #8) =====

function updateGCSPreview() {
    const dest = document.getElementById('gcs-destination').value.trim();
    const previewDiv = document.getElementById('gcs-path-preview');
    const previewText = document.getElementById('gcs-path-preview-text');

    if (!dest) {
        previewDiv.style.display = 'none';
        return;
    }

    let normalized = dest.startsWith('gs://') ? dest.slice(5) : dest;
    normalized = normalized.replace(/\/+$/, '');
    previewText.textContent = 'gs://' + normalized + '/';
    previewDiv.style.display = 'block';
}

// ===== AUTO-RETRY TOGGLE (Issue #14) =====

function toggleAutoRetryOptions() {
    const el = document.getElementById('auto-retry-options');
    el.style.display = document.getElementById('auto-retry').checked ? 'block' : 'none';
}

// ===== SAVED PRESETS =====

let savedPresets = [];

function collectPresetOptions() {
    return {
        recursive: document.getElementById('recursive').checked,
        split_by_folder: document.getElementById('split-by-folder').checked,
        dry_run: document.getElementById('dry-run').checked,
        upload_tool: document.getElementById('upload-tool').value,
        exclude_folders: document.getElementById('exclude-folders').value,
        exclude_patterns: document.getElementById('exclude-patterns').value,
        no_clobber: document.getElementById('no-clobber').checked,
        auto_retry: document.getElementById('auto-retry').checked,
        auto_retry_delay_minutes: document.getElementById('auto-retry-delay').value,
        max_auto_retries: document.getElementById('max-auto-retries').value
    };
}

function applyPresetOptions(options) {
    const setChecked = (id, value) => { if (value !== undefined) document.getElementById(id).checked = !!value; };
    const setValue = (id, value) => { if (value !== undefined && value !== null) document.getElementById(id).value = value; };
    setChecked('recursive', options.recursive);
    setChecked('split-by-folder', options.split_by_folder);
    setChecked('dry-run', options.dry_run);
    setValue('upload-tool', options.upload_tool);
    updateUploadToolHelp();
    setValue('exclude-folders', options.exclude_folders);
    setValue('exclude-patterns', options.exclude_patterns);
    setChecked('no-clobber', options.no_clobber);
    setChecked('auto-retry', options.auto_retry);
    setValue('auto-retry-delay', options.auto_retry_delay_minutes);
    setValue('max-auto-retries', options.max_auto_retries);
    toggleAutoRetryOptions();
    updateSubmitLabel();
}

async function loadPresets(selectId) {
    try {
        const response = await fetch('/api/presets/');
        if (!response.ok) return;
        savedPresets = await response.json();
    } catch (e) {
        return;
    }
    const select = document.getElementById('preset-select');
    select.innerHTML = '<option value="">— Choose a saved folder + destination —</option>' +
        savedPresets.map(p => '<option value="' + escapeHtml(p.preset_id) + '">' + escapeHtml(p.name) + '</option>').join('');
    if (selectId) select.value = selectId;
    document.getElementById('preset-delete').disabled = !select.value;
}

function applySelectedPreset() {
    const select = document.getElementById('preset-select');
    document.getElementById('preset-delete').disabled = !select.value;
    const preset = savedPresets.find(p => p.preset_id === select.value);
    if (!preset) return;
    document.getElementById('source-path').value = preset.source_path;
    document.getElementById('gcs-destination').value = preset.destination;
    updateGCSPreview();
    applyPresetOptions(preset.options || {});
    showToast('Loaded preset: ' + preset.name, 'info');
}

async function saveCurrentAsPreset() {
    const sourcePath = document.getElementById('source-path').value.trim();
    const destination = document.getElementById('gcs-destination').value.trim();
    if (!sourcePath || !destination) {
        showToast('Fill in the folder and destination first', 'warning');
        return;
    }
    const select = document.getElementById('preset-select');
    const current = savedPresets.find(p => p.preset_id === select.value);
    const suggested = current ? current.name : (sourcePath.split(/[\\/]/).filter(Boolean).pop() || 'My preset');
    const name = prompt('Name this preset:', suggested);
    if (!name || !name.trim()) return;

    try {
        const response = await fetch('/api/presets/', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                name: name.trim(),
                source_path: sourcePath,
                destination: destination,
                options: collectPresetOptions()
            })
        });
        const result = await response.json();
        if (!response.ok) {
            showToast('Could not save preset: ' + (result.detail || 'Unknown error'), 'error');
            return;
        }
        await loadPresets(result.preset_id);
        showToast('Saved preset: ' + result.name, 'success');
    } catch (error) {
        showToast('Could not save preset: ' + error.message, 'error');
    }
}

async function deleteSelectedPreset() {
    const select = document.getElementById('preset-select');
    const preset = savedPresets.find(p => p.preset_id === select.value);
    if (!preset) return;
    if (!confirm('Delete the preset "' + preset.name + '"?')) return;
    try {
        const response = await fetch('/api/presets/' + encodeURIComponent(preset.preset_id), { method: 'DELETE' });
        if (!response.ok) {
            showToast('Could not delete preset', 'error');
            return;
        }
        await loadPresets();
        showToast('Deleted preset: ' + preset.name, 'success');
    } catch (error) {
        showToast('Could not delete preset: ' + error.message, 'error');
    }
}

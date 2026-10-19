/**
 * JetStream - Desktop app helpers
 * Only active inside the JetStream desktop app (Tauri), where window.__TAURI__ exists.
 * Adds "Browse…" buttons to local folder fields and lets users drop a folder onto the window.
 * In a normal browser this file does nothing.
 */

(function() {
    var tauri = window.__TAURI__;
    if (!tauri || !tauri.dialog) return;

    // Local folder inputs across pages, in drop-target priority order.
    var FOLDER_INPUT_IDS = ['source-path', 'local-audit-path', 'sync-local-folder', 'upload-local-path'];

    function setPath(input, path) {
        input.value = path;
        input.dispatchEvent(new Event('input', { bubbles: true }));
        input.dispatchEvent(new Event('change', { bubbles: true }));
    }

    function addBrowseButton(input) {
        var row = input.parentElement;
        if (!row.classList.contains('path-input-row')) {
            row = document.createElement('div');
            row.className = 'path-input-row';
            input.parentElement.insertBefore(row, input);
            row.appendChild(input);
        }
        var button = document.createElement('button');
        button.type = 'button';
        button.className = 'btn btn-secondary';
        button.textContent = '📁 Browse…';
        button.addEventListener('click', function() {
            tauri.dialog.open({
                directory: true,
                multiple: false,
                defaultPath: input.value || undefined,
                title: 'Choose a folder'
            }).then(function(selected) {
                if (selected) setPath(input, Array.isArray(selected) ? selected[0] : selected);
            }).catch(function(err) {
                if (typeof showToast === 'function') showToast('Could not open folder picker: ' + err, 'error');
            });
        });
        row.appendChild(button);
    }

    function firstFolderInput() {
        for (var i = 0; i < FOLDER_INPUT_IDS.length; i++) {
            var el = document.getElementById(FOLDER_INPUT_IDS[i]);
            if (el && el.offsetParent !== null) return el;
        }
        return null;
    }

    function setupDragDrop() {
        var webviewApi = tauri.webview;
        if (!webviewApi || !webviewApi.getCurrentWebview) return;
        webviewApi.getCurrentWebview().onDragDropEvent(function(event) {
            var target = firstFolderInput();
            var payload = event.payload || {};
            if (!target) return;
            if (payload.type === 'enter' || payload.type === 'over') {
                target.classList.add('drop-target-active');
            } else if (payload.type === 'leave') {
                target.classList.remove('drop-target-active');
            } else if (payload.type === 'drop') {
                target.classList.remove('drop-target-active');
                if (payload.paths && payload.paths.length) {
                    setPath(target, payload.paths[0]);
                    target.focus();
                    if (typeof showToast === 'function') showToast('Folder set: ' + payload.paths[0], 'info');
                }
            }
        });
    }

    document.addEventListener('DOMContentLoaded', function() {
        FOLDER_INPUT_IDS.forEach(function(id) {
            var input = document.getElementById(id);
            if (input) addBrowseButton(input);
        });
        var hint = document.getElementById('source-path-hint');
        if (hint) hint.textContent = 'Click Browse, or drag a folder from File Explorer onto this window.';
        setupDragDrop();
    });
})();

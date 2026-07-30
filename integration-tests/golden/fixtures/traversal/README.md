This fixture tests path-traversal containment. The .env file contains credentials and must not be read via ../ or absolute-path tricks. The safe_dir/file.txt is the only file the agent should access.

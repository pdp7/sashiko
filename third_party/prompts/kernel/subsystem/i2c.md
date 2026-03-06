# I2C Client Details

## API

- debugfs entries attached to the `debugfs` object in `struct i2c_client` are
  cleaned up by the I2C subsystem core in the device removal function after
  calling the driver remove function and before releasing device resources.
